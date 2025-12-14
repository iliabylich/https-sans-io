use crate::{FSM, Request, Response, Wants};
use anyhow::Result;
use rustls::pki_types::ServerName;
use std::{
    io::{Read as _, Write as _},
    net::TcpStream,
};

pub struct BlockingConnection;

impl BlockingConnection {
    pub fn get(hostname: &str, port: u16, path: &str) -> Result<Response> {
        let mut fsm = {
            let server_name = ServerName::try_from(hostname)?.to_owned();

            let mut request = Request::get(path);
            request.add_header("Host", hostname);
            request.add_header("Connection", "close");

            FSM::new(server_name, request)?
        };

        let mut sock = TcpStream::connect(format!("{hostname}:{port}"))?;

        loop {
            let action = fsm.wants()?;

            match action {
                Wants::Read(buf) => {
                    let read = sock.read(buf)?;
                    fsm.done_reading(read);
                }
                Wants::Write(buf) => {
                    let written = sock.write(buf)?;
                    fsm.done_writing(written);
                }
                Wants::Done(response) => {
                    return Ok(response);
                }
            }
        }
    }
}
