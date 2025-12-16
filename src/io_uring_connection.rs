use crate::{FSM, Request, Response, Wants};
use anyhow::{Result, bail};
use libc::{AF_INET, SOCK_STREAM, addrinfo, freeaddrinfo, gai_strerror, sockaddr, sockaddr_in};
use rustls::pki_types::ServerName;
use std::{
    collections::HashSet,
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr::null_mut,
};

#[derive(Default)]
enum State {
    Initialized {
        addr: sockaddr_in,
    },
    Connecting {
        fd: i32,
        addr: sockaddr_in,
    },
    Connected {
        fd: i32,
    },
    #[default]
    None,
}

pub struct IoUringConnection {
    fsm: FSM,
    state: State,
    socket_user_data: u64,
    connect_user_data: u64,
    read_user_data: u64,
    write_user_data: u64,
    pending: HashSet<u64>,
}

impl IoUringConnection {
    pub fn get(
        hostname: &str,
        port: u16,
        path: &str,
        socket_user_data: u64,
        connect_user_data: u64,
        read_user_data: u64,
        write_user_data: u64,
    ) -> Result<Self> {
        let fsm = {
            let server_name = ServerName::try_from(hostname)?.to_owned();

            let mut request = Request::get(path);
            request.add_header("Host", hostname);
            request.add_header("Connection", "close");

            FSM::new(server_name, request)?
        };

        let mut addr = getaddrinfo(hostname)?;
        addr.sin_port = port.to_be();

        Ok(Self {
            fsm,
            state: State::Initialized { addr },
            socket_user_data,
            connect_user_data,
            read_user_data,
            write_user_data,
            pending: HashSet::new(),
        })
    }

    pub fn next_sqe(&mut self) -> Result<(Option<Sqe>, Option<Response>)> {
        let sqe;

        match &self.state {
            State::Initialized { .. } => {
                sqe = socket_sqe(self.socket_user_data);
            }
            State::Connecting { fd, addr, .. } => {
                sqe = connect_sqe(*fd, addr, self.connect_user_data);
            }
            State::Connected { fd } => match self.fsm.wants()? {
                Wants::Read(buf) => {
                    sqe = read_sqe(*fd, buf, self.read_user_data);
                }
                Wants::Write(buf) => {
                    sqe = write_sqe(*fd, buf, self.write_user_data);
                }
                Wants::Done(response) => {
                    return Ok((None, Some(response)));
                }
            },
            State::None => unreachable!(),
        }

        if self.pending.contains(&sqe.user_data()) {
            return Ok((None, None));
        }
        self.pending.insert(sqe.user_data());

        Ok((Some(sqe), None))
    }

    fn take_state(&mut self) -> State {
        std::mem::take(&mut self.state)
    }

    pub fn process_cqe(&mut self, cqe: Cqe) -> Result<()> {
        self.pending.remove(&cqe.user_data);

        match cqe.user_data {
            data if data == self.socket_user_data => {
                let fd = cqe.result;
                assert!(fd > 0);

                let State::Initialized { addr } = self.take_state() else {
                    panic!("malformed state")
                };

                self.state = State::Connecting { fd, addr };
            }
            data if data == self.connect_user_data => {
                assert!(cqe.result >= 0);

                let State::Connecting { fd, .. } = self.take_state() else {
                    panic!("malformed state")
                };

                self.state = State::Connected { fd };
            }
            data if data == self.read_user_data => {
                let read = cqe.result;
                assert!(read >= 0);
                let read = read as usize;

                self.fsm.done_reading(read);
            }
            data if data == self.write_user_data => {
                let written = cqe.result;
                assert!(written >= 0);
                let written = written as usize;

                self.fsm.done_writing(written);
            }

            _ => {}
        }

        Ok(())
    }
}

fn getaddrinfo(hostname: &str) -> Result<sockaddr_in> {
    let node = CString::new(hostname)?;
    let mut hints = unsafe { MaybeUninit::<addrinfo>::zeroed().assume_init() };
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;

    let mut result = null_mut();

    let res = unsafe { libc::getaddrinfo(node.as_ptr(), null_mut(), &hints, &mut result) };
    if res != 0 {
        bail!("{}", unsafe { CStr::from_ptr(gai_strerror(res)) }.to_str()?)
    }

    let mut rp = result;
    while !rp.is_null() {
        if unsafe { *rp }.ai_family == AF_INET {
            let ip = unsafe { *(*rp).ai_addr.cast::<sockaddr_in>() };
            unsafe { freeaddrinfo(rp) }
            return Ok(ip);
        }

        rp = (unsafe { *rp }).ai_next;
    }
    unsafe { freeaddrinfo(rp) }

    bail!("failed to resolve DNS name: {hostname}")
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum Sqe {
    Socket {
        domain: i32,
        socket_type: i32,
        protocol: i32,
        user_data: u64,
    },

    Connect {
        fd: i32,
        addr: *const sockaddr,
        addrlen: u32,
        user_data: u64,
    },

    Write {
        fd: i32,
        buf: *const u8,
        len: u32,
        user_data: u64,
    },

    Read {
        fd: i32,
        buf: *mut u8,
        len: u32,
        user_data: u64,
    },
}

impl Sqe {
    fn user_data(self) -> u64 {
        match self {
            Self::Socket { user_data, .. }
            | Self::Connect { user_data, .. }
            | Self::Write { user_data, .. }
            | Self::Read { user_data, .. } => user_data,
        }
    }
}

fn socket_sqe(user_data: u64) -> Sqe {
    Sqe::Socket {
        domain: AF_INET,
        socket_type: SOCK_STREAM,
        protocol: 0,
        user_data,
    }
}

fn connect_sqe(fd: i32, addr: *const sockaddr_in, user_data: u64) -> Sqe {
    Sqe::Connect {
        fd,
        addr: addr.cast::<sockaddr>(),
        addrlen: std::mem::size_of::<sockaddr_in>() as u32,
        user_data,
    }
}

fn write_sqe(fd: i32, buf: &[u8], user_data: u64) -> Sqe {
    Sqe::Write {
        fd,
        buf: buf.as_ptr(),
        len: buf.len() as u32,
        user_data,
    }
}

fn read_sqe(fd: i32, buf: &mut [u8], user_data: u64) -> Sqe {
    Sqe::Read {
        fd,
        buf: buf.as_mut_ptr(),
        len: buf.len() as u32,
        user_data,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Cqe {
    pub result: i32,
    pub user_data: u64,
}
