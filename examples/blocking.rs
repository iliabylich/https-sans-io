use std::{
    io::{Read as _, Write},
    net::TcpStream,
};

use anyhow::Result;
use https_sansio::{FSM, Request, Wants};
use rustls::pki_types::ServerName;

const HOSTNAME: &str = "myip.ibylich.dev";
const PORT: u16 = 443;

fn main() -> Result<()> {
    let mut fsm = {
        let server_name = ServerName::try_from(HOSTNAME)?;

        let mut request = Request::get("/");
        request.add_header("Host", HOSTNAME);
        request.add_header("Connection", "close");

        FSM::new(server_name, request)?
    };

    let mut sock = TcpStream::connect(format!("{HOSTNAME}:{PORT}"))?;

    loop {
        let action = fsm.wants()?;

        match action {
            Wants::Read(buf) => {
                eprintln!("Starting sock read");
                let read = sock.read(buf)?;
                eprintln!("received {read}B of data");
                fsm.done_reading(read);
            }
            Wants::Write(buf) => {
                eprintln!("Starting sock write");
                let written = sock.write(buf)?;
                eprintln!("sent {written}B of data");
                fsm.done_writing(written);
            }
            Wants::Done(response) => {
                eprintln!("Got response: {:?}", response);
                break;
            }
        }
    }

    Ok(())
}
