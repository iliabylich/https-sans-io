use crate::{FSM, Request, Response, Wants};
use anyhow::{Result, bail};
use io_uring::{cqueue::Entry as Cqe, opcode, squeue::Entry as Sqe, types};
use libc::{AF_INET, SOCK_STREAM, addrinfo, freeaddrinfo, gai_strerror, sockaddr, sockaddr_in};
use rustls::pki_types::ServerName;
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr::null_mut,
};

#[derive(Default)]
pub enum IoUringConnection {
    Initialized {
        fsm: FSM,
        addr: sockaddr_in,
    },
    Connecting {
        fsm: FSM,
        fd: i32,
        addr: sockaddr_in,
    },
    Connected {
        fsm: FSM,
        fd: i32,
    },
    #[default]
    None,
}

pub enum SqeOrResponse {
    Sqe(Sqe),
    Response(Response),
}

impl IoUringConnection {
    pub fn get(hostname: &str, port: u16, path: &str) -> Result<Self> {
        let fsm = {
            let server_name = ServerName::try_from(hostname)?.to_owned();

            let mut request = Request::get(path);
            request.add_header("Host", hostname);
            request.add_header("Connection", "close");

            FSM::new(server_name, request)?
        };

        let mut addr = getaddrinfo(hostname)?;
        addr.sin_port = port.to_be();

        Ok(Self::Initialized { fsm, addr })
    }

    pub fn next_sqe(&mut self) -> Result<SqeOrResponse> {
        match self {
            Self::Initialized { .. } => Ok(SqeOrResponse::Sqe(socket_sqe())),
            Self::Connecting { fd, addr, .. } => Ok(SqeOrResponse::Sqe(connect_sqe(*fd, addr))),
            Self::Connected { fsm, fd } => match fsm.wants()? {
                Wants::Read(buf) => Ok(SqeOrResponse::Sqe(read_sqe(*fd, buf))),
                Wants::Write(buf) => Ok(SqeOrResponse::Sqe(write_sqe(*fd, buf))),
                Wants::Done(response) => Ok(SqeOrResponse::Response(response)),
            },
            Self::None => unreachable!(),
        }
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }

    pub fn process_cqe(&mut self, cqe: Cqe) -> Result<()> {
        match cqe.user_data() {
            SOCKET_USER_DATA => {
                let fd = cqe.result();
                assert!(fd > 0);

                let Self::Initialized { fsm, addr } = self.take() else {
                    panic!("malformed state")
                };

                *self = Self::Connecting { fsm, fd, addr };
            }
            CONNECT_USER_DATA => {
                assert!(cqe.result() >= 0);

                let Self::Connecting { fsm, fd, .. } = self.take() else {
                    panic!("malformed state")
                };

                *self = Self::Connected { fsm, fd };
            }
            READ_USER_DATA => {
                let read = cqe.result();
                assert!(read >= 0);
                let read = read as usize;

                let Self::Connected { fsm, .. } = self else {
                    panic!("malformed state");
                };
                fsm.done_reading(read);
            }
            WRITE_USER_DATA => {
                let written = cqe.result();
                assert!(written >= 0);
                let written = written as usize;

                let Self::Connected { fsm, .. } = self else {
                    panic!("malformed state");
                };
                fsm.done_writing(written);
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

const SOCKET_USER_DATA: u64 = 1;
const CONNECT_USER_DATA: u64 = 2;
const READ_USER_DATA: u64 = 3;
const WRITE_USER_DATA: u64 = 4;

fn socket_sqe() -> Sqe {
    opcode::Socket::new(AF_INET, SOCK_STREAM, 0)
        .build()
        .user_data(SOCKET_USER_DATA)
}

fn connect_sqe(fd: i32, addr: *const sockaddr_in) -> Sqe {
    opcode::Connect::new(
        types::Fd(fd),
        addr.cast::<sockaddr>(),
        std::mem::size_of::<sockaddr_in>() as u32,
    )
    .build()
    .user_data(CONNECT_USER_DATA)
}

fn write_sqe(fd: i32, buf: &[u8]) -> Sqe {
    opcode::Write::new(types::Fd(fd), buf.as_ptr(), buf.len() as u32)
        .build()
        .user_data(WRITE_USER_DATA)
}

fn read_sqe(fd: i32, buf: &mut [u8]) -> Sqe {
    opcode::Read::new(types::Fd(fd), buf.as_mut_ptr(), buf.len() as u32)
        .build()
        .user_data(READ_USER_DATA)
}
