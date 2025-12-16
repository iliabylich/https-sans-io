use anyhow::Result;

#[cfg(feature = "blocking")]
fn main() -> Result<()> {
    println!("Blocking version");

    use https_sans_io::BlockingConnection;
    let response = BlockingConnection::get("myip.ibylich.dev", 443, "/")?;
    println!("Response: {response:?}");
    Ok(())
}

#[cfg(feature = "poll")]
fn main() -> Result<()> {
    println!("Poll version");

    use https_sans_io::{EventsOrResponse, PollConnection};
    let mut conn = PollConnection::get("myip.ibylich.dev", 443, "/")?;

    use libc::{POLLERR, POLLIN, POLLOUT, poll, pollfd};
    use std::os::fd::AsRawFd;

    let mut fds = [pollfd {
        fd: conn.as_raw_fd(),
        events: POLLIN | POLLOUT,
        revents: 0,
    }];

    fn do_poll(fds: &mut [pollfd; 1]) -> (bool, bool) {
        let res = unsafe { poll(fds.as_mut_ptr(), 1, -1) };
        assert!(res == 1);
        let readable = fds[0].revents & POLLIN != 0;
        let writable = fds[0].revents & POLLOUT != 0;
        assert_eq!(fds[0].revents & POLLERR, 0);
        (readable, writable)
    }

    let response = loop {
        match conn.events()? {
            EventsOrResponse::Events(events) => {
                fds[0].events = events;
            }
            EventsOrResponse::Response(response) => break response,
        }
        let (readable, writable) = do_poll(&mut fds);

        if let Some(response) = conn.poll(readable, writable)? {
            break response;
        };
    };

    println!("Response: {response:?}");
    Ok(())
}

#[cfg(feature = "io-uring-with-dep")]
fn main() -> Result<()> {
    println!("io_uring version");

    use https_sans_io::{Cqe, IoUringConnection, Sqe, SqeOrResponse};
    use io_uring::{IoUring, opcode, types};

    let mut ring = IoUring::new(10)?;

    const SOCKET_USER_DATA: u64 = 1;
    const CONNECT_USER_DATA: u64 = 2;
    const READ_USER_DATA: u64 = 3;
    const WRITE_USER_DATA: u64 = 4;
    let mut conn = IoUringConnection::get(
        "myip.ibylich.dev",
        443,
        "/",
        SOCKET_USER_DATA,
        CONNECT_USER_DATA,
        READ_USER_DATA,
        WRITE_USER_DATA,
    )?;

    fn map_sqe(sqe: Sqe) -> io_uring::squeue::Entry {
        match sqe {
            Sqe::Socket {
                domain,
                socket_type,
                protocol,
                user_data,
            } => opcode::Socket::new(domain, socket_type, protocol)
                .build()
                .user_data(user_data),
            Sqe::Connect {
                fd,
                addr,
                addrlen,
                user_data,
            } => opcode::Connect::new(types::Fd(fd), addr, addrlen)
                .build()
                .user_data(user_data),
            Sqe::Write {
                fd,
                buf,
                len,
                user_data,
            } => opcode::Write::new(types::Fd(fd), buf, len)
                .build()
                .user_data(user_data),
            Sqe::Read {
                fd,
                buf,
                len,
                user_data,
            } => opcode::Read::new(types::Fd(fd), buf, len)
                .build()
                .user_data(user_data),
        }
    }

    fn map_cqe(cqe: io_uring::cqueue::Entry) -> Cqe {
        Cqe {
            result: cqe.result(),
            user_data: cqe.user_data(),
        }
    }

    let response = loop {
        match conn.next_sqe()? {
            SqeOrResponse::Sqe(sqe) => {
                unsafe { ring.submission().push(&map_sqe(sqe))? };
            }
            SqeOrResponse::Response(response) => break response,
        }

        ring.submit_and_wait(1)?;

        while let Some(cqe) = ring.completion().next() {
            conn.process_cqe(map_cqe(cqe))?;
        }
    };

    println!("Response: {response:?}");
    Ok(())
}
