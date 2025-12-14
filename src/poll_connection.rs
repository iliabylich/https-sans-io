use crate::{FSM, Request, Response, Wants};
use anyhow::Result;
use libc::{POLLIN, POLLOUT};
use rustls::pki_types::ServerName;
use std::{
    io::{ErrorKind, Read, Write},
    net::TcpStream,
    os::fd::AsRawFd,
};

pub struct PollConnection {
    fsm: FSM,
    sock: TcpStream,
    response: Option<Response>,
    done: bool,
}

pub enum EventsOrResponse {
    Events(i16),
    Response(Response),
}

impl PollConnection {
    pub fn get(hostname: &str, port: u16, path: &str) -> Result<Self> {
        let fsm = {
            let server_name = ServerName::try_from(hostname)?.to_owned();

            let mut request = Request::get(path);
            request.add_header("Host", hostname);
            request.add_header("Connection", "close");

            FSM::new(server_name, request)?
        };

        let sock = TcpStream::connect(format!("{hostname}:{port}"))?;
        sock.set_nonblocking(true)?;

        Ok(Self {
            fsm,
            sock,
            response: None,
            done: false,
        })
    }

    pub fn events(&mut self) -> Result<EventsOrResponse> {
        match self.fsm.wants()? {
            Wants::Read(_) => Ok(EventsOrResponse::Events(POLLIN)),
            Wants::Write(_) => Ok(EventsOrResponse::Events(POLLOUT)),
            Wants::Done(response) => Ok(EventsOrResponse::Response(response)),
        }
    }

    pub fn poll(&mut self, readable: bool, writable: bool) -> Result<Option<Response>> {
        if self.done {
            return Ok(self.response.take());
        }

        assert!(
            !(readable && writable),
            "exactly one of readable/writable must be set, got both: {readable}/{writable}"
        );

        if readable {
            self.poll_read()
        } else if writable {
            self.poll_write()
        } else {
            panic!("at least one of readable/writable must be set: {readable}/{writable}");
        }
    }

    fn poll_read(&mut self) -> Result<Option<Response>> {
        loop {
            match self.fsm.wants()? {
                Wants::Read(buf) => match self.sock.read(buf) {
                    Ok(read) => self.fsm.done_reading(read),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(None),
                    Err(err) => return Err(err.into()),
                },
                Wants::Done(response) => {
                    self.done = true;
                    return Ok(Some(response));
                }
                Wants::Write(_) => return Ok(None),
            }
        }
    }

    fn poll_write(&mut self) -> Result<Option<Response>> {
        loop {
            match self.fsm.wants()? {
                Wants::Write(buf) => match self.sock.write(buf) {
                    Ok(written) => self.fsm.done_writing(written),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(None),
                    Err(err) => return Err(err.into()),
                },
                Wants::Done(response) => {
                    self.done = true;
                    return Ok(Some(response));
                }
                Wants::Read(_) => return Ok(None),
            }
        }
    }
}

impl AsRawFd for PollConnection {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.sock.as_raw_fd()
    }
}
