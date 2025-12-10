use std::{
    io::{Read as _, Write},
    net::TcpStream,
    sync::Arc,
};

use anyhow::Result;
use https_sansio::{FSM, HttpsAction};
use rustls::{ClientConfig, RootCertStore, version::TLS13};

const SERVER_NAME: &str = "myip.ibylich.dev";
const PORT: u16 = 443;

fn main() -> Result<()> {
    let root_store = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.into(),
    };

    let config = ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let config = Arc::new(config);

    let mut fsm = FSM::new(Arc::clone(&config), SERVER_NAME.try_into()?)?;

    let mut sock = TcpStream::connect(format!("{SERVER_NAME}:{PORT}"))?;

    loop {
        let action = fsm.next_action()?;

        match action {
            HttpsAction::Read(buf) => {
                eprintln!("Starting sock read");
                let read = sock.read(buf)?;
                eprintln!("received {read}B of data");
                fsm.done_reading(read);
            }
            HttpsAction::Write(buf) => {
                eprintln!("Starting sock write");
                let written = sock.write(buf)?;
                eprintln!("sent {written}B of data");
                fsm.done_writing(written);
            }
            HttpsAction::Done(response) => {
                eprintln!("Got response: {}", String::from_utf8(response).unwrap());
                break;
            }
        }
    }

    Ok(())
}
