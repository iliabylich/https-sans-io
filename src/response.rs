use anyhow::{Context as _, Result};
use std::collections::HashMap;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Response {
    pub(crate) fn parse(data: Vec<u8>) -> Result<Self> {
        let data = String::from_utf8(data)?;
        let (pre, body) = data
            .split_once("\r\n\r\n")
            .context("no separator between headers and body")?;
        let (status, headers) = pre
            .split_once("\r\n")
            .context("no separator between status line and headers")?;

        let status = status
            .split(" ")
            .nth(1)
            .context("malformed status line")?
            .parse::<u16>()
            .context("non-numeric HTTP status")?;

        let headers = {
            let mut out = HashMap::new();
            for line in headers.split("\r\n") {
                let (name, value) = line.split_once(": ").context("malformed header")?;
                out.insert(name.to_string(), value.to_string());
            }
            out
        };

        Ok(Self {
            status,
            headers,
            body: body.to_string(),
        })
    }
}
