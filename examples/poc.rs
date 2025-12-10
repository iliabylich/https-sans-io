use anyhow::{Context as _, Result};
use rustls::client::UnbufferedClientConnection;
use rustls::unbuffered::{
    AppDataRecord, ConnectionState, EncodeError, EncryptError, InsufficientSizeError,
    UnbufferedStatus,
};
use rustls::version::TLS13;
use rustls::{ClientConfig, RootCertStore};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

fn main() -> Result<()> {
    let root_store = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.into(),
    };

    let config = ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let config = Arc::new(config);

    let mut incoming_tls = vec![0; INCOMING_TLS_INITIAL_BUFSIZE];
    let mut outgoing_tls = vec![0; OUTGOING_TLS_INITIAL_BUFSIZE];

    let response = converse(&config, &mut incoming_tls, &mut outgoing_tls)?;
    println!("{}", String::from_utf8_lossy(&response));

    Ok(())
}

fn converse(
    config: &Arc<ClientConfig>,
    incoming_tls: &mut Vec<u8>,
    outgoing_tls: &mut Vec<u8>,
) -> Result<Vec<u8>> {
    let mut conn = UnbufferedClientConnection::new(config.clone(), SERVER_NAME.try_into()?)?;
    let mut sock = TcpStream::connect(format!("{SERVER_NAME}:{PORT}"))?;

    let mut incoming_start = 0;
    let mut incoming_end = 0;
    let mut outgoing_end = 0;

    let mut we_closed = false;
    let mut sent_request = false;
    let mut received_response = false;

    let mut response = vec![];

    loop {
        println!(
            "Process TLS records: {:?}",
            &incoming_tls[incoming_start..incoming_end].len()
        );

        let UnbufferedStatus { discard, state } =
            conn.process_tls_records(&mut incoming_tls[incoming_start..incoming_end]);

        incoming_start += discard;

        let state = state.context("malformed internal state")?;

        match dbg!(state) {
            ConnectionState::ReadTraffic(mut state) => {
                while let Some(res) = state.next_record() {
                    let AppDataRecord {
                        discard: new_discard,
                        payload,
                    } = res.context("failed to get AppDataRecord")?;

                    incoming_start += new_discard;

                    println!("Received {}", String::from_utf8_lossy(payload));

                    response.extend_from_slice(payload);

                    received_response = true;
                }
            }

            ConnectionState::EncodeTlsData(mut state) => {
                let written = match state.encode(&mut outgoing_tls[outgoing_end..]) {
                    Ok(written) => written,

                    Err(EncodeError::InsufficientSize(InsufficientSizeError { required_size })) => {
                        let new_len = outgoing_end + required_size;
                        outgoing_tls.resize(new_len, 0);
                        state.encode(&mut outgoing_tls[outgoing_end..])?
                    }

                    Err(e) => {
                        return Err(e.into());
                    }
                };

                outgoing_end += written;
            }

            ConnectionState::TransmitTlsData(mut state) => {
                if let Some(mut may_encrypt) = state.may_encrypt_app_data() {
                    if !sent_request {
                        let request = make_request();
                        let written = may_encrypt
                            .encrypt(&request, &mut outgoing_tls[outgoing_end..])
                            .context("encrypted request does not fit in `outgoing_tls`")?;
                        outgoing_end += written;
                        sent_request = true;
                        eprintln!("queued HTTP request");
                    }
                }

                send_tls(&mut sock, outgoing_tls, &mut outgoing_end)?;
                state.done();
            }

            ConnectionState::BlockedHandshake { .. } => {
                resize_incoming_if_needed(incoming_tls, incoming_end);
                recv_tls(&mut sock, incoming_tls, &mut incoming_end)?;
            }

            ConnectionState::WriteTraffic(mut may_encrypt) => {
                if !sent_request {
                    panic!("dead branch?");
                    // let request = make_request();
                    // let written = may_encrypt
                    //     .encrypt(&request, &mut outgoing_tls[outgoing_end..])
                    //     .context("encrypted request does not fit in `outgoing_tls`")?;
                    // outgoing_end += written;
                    // sent_request = true;
                    // eprintln!("queued HTTP request");

                    // send_tls(&mut sock, outgoing_tls, &mut outgoing_end)?;
                    // resize_incoming_if_needed(incoming_tls, incoming_end);
                    // recv_tls(&mut sock, incoming_tls, &mut incoming_end)?;
                } else if !received_response {
                    // this happens in the TLS 1.3 case. the app-data was sent in the preceding
                    // `TransmitTlsData` state. the server should have already written a
                    // response which we can read out from the socket
                    resize_incoming_if_needed(incoming_tls, incoming_end);
                    recv_tls(&mut sock, incoming_tls, &mut incoming_end)?;
                } else if !we_closed {
                    let written =
                        match may_encrypt.queue_close_notify(&mut outgoing_tls[outgoing_end..]) {
                            Ok(written) => written,

                            Err(EncryptError::InsufficientSize(InsufficientSizeError {
                                required_size,
                            })) => {
                                let new_len = outgoing_end + required_size;
                                outgoing_tls.resize(new_len, 0);
                                may_encrypt.queue_close_notify(&mut outgoing_tls[outgoing_end..])?
                            }

                            Err(e) => {
                                return Err(e.into());
                            }
                        };

                    outgoing_end += written;

                    send_tls(&mut sock, outgoing_tls, &mut outgoing_end)?;
                    we_closed = true;
                } else {
                    resize_incoming_if_needed(incoming_tls, incoming_end);

                    recv_tls(&mut sock, incoming_tls, &mut incoming_end)?;
                }
            }

            ConnectionState::PeerClosed => {}

            ConnectionState::Closed => {
                break;
            }

            _ => unreachable!(),
        }
    }

    assert!(sent_request);
    assert!(received_response);
    assert_eq!(incoming_start, incoming_end);
    assert_eq!(0, outgoing_end);

    Ok(response)
}

fn make_request() -> Vec<u8> {
    format!("GET / HTTP/1.1\r\nHost: {SERVER_NAME}\r\nConnection: close\r\nAccept-Encoding: identity\r\n\r\n").into_bytes()
}

fn resize_incoming_if_needed(incoming_tls: &mut Vec<u8>, incoming_end: usize) {
    if incoming_end == incoming_tls.len() {
        let new_len = incoming_tls.len() + INCOMING_TLS_BUFSIZE;
        incoming_tls.resize(new_len, 0);
        eprintln!("grew buffer to {} bytes", new_len);
    }
}

fn recv_tls(
    sock: &mut TcpStream,
    incoming_tls: &mut Vec<u8>,
    incoming_end: &mut usize,
) -> Result<()> {
    let read = sock.read(&mut incoming_tls[*incoming_end..])?;
    eprintln!("received {read}B of data");
    *incoming_end += read;
    Ok(())
}

fn send_tls(sock: &mut TcpStream, outgoing_tls: &[u8], outgoing_end: &mut usize) -> Result<()> {
    sock.write_all(&outgoing_tls[..*outgoing_end])?;
    eprintln!("sent {outgoing_end}B of data");
    *outgoing_end = 0;
    Ok(())
}

const SERVER_NAME: &str = "myip.ibylich.dev";
const PORT: u16 = 443;

const KB: usize = 1024;
const INCOMING_TLS_INITIAL_BUFSIZE: usize = KB;
const INCOMING_TLS_BUFSIZE: usize = 16 * KB;
const OUTGOING_TLS_INITIAL_BUFSIZE: usize = KB;
