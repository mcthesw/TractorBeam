use std::{
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

use super::InternalTestReportError;

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InternalTestUploadReceipt {
    pub status_code: u16,
    pub response_body: String,
}

pub(super) fn post_zip(
    endpoint: &str,
    token: &str,
    report_id: &str,
    test_run_id: &str,
    body: &[u8],
) -> Result<InternalTestUploadReceipt, InternalTestReportError> {
    let endpoint = HttpEndpoint::parse(endpoint)?;
    let mut stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))?;
    stream.set_read_timeout(Some(UPLOAD_TIMEOUT))?;
    stream.set_write_timeout(Some(UPLOAD_TIMEOUT))?;
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\nX-Basement-Bridge-Report-Id: {}\r\nX-Basement-Bridge-Test-Run-Id: {}\r\n\r\n",
        endpoint.path,
        endpoint.host_header,
        token,
        body.len(),
        report_id,
        test_run_id
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (status_code, response_body) = parse_http_response(&response)?;
    if !(200..300).contains(&status_code) {
        return Err(InternalTestReportError::UploadRejected {
            status_code,
            body: response_body,
        });
    }
    Ok(InternalTestUploadReceipt {
        status_code,
        response_body,
    })
}

#[derive(Debug, Eq, PartialEq)]
struct HttpEndpoint {
    host: String,
    host_header: String,
    port: u16,
    path: String,
}

impl HttpEndpoint {
    fn parse(value: &str) -> Result<Self, InternalTestReportError> {
        let Some(rest) = value.trim().strip_prefix("http://") else {
            return Err(InternalTestReportError::UnsupportedUploadEndpoint);
        };
        let (authority, path) = rest.split_once('/').map_or((rest, "/"), |(host, path)| {
            (host, &rest[rest.len() - path.len() - 1..])
        });
        if authority.is_empty() {
            return Err(InternalTestReportError::InvalidUploadEndpoint(
                "host is required".to_owned(),
            ));
        }
        let (host, port) = authority.rsplit_once(':').map_or_else(
            || Ok((authority.to_owned(), 80)),
            |(host, port)| {
                if host.is_empty() {
                    return Err(InternalTestReportError::InvalidUploadEndpoint(
                        "host is required".to_owned(),
                    ));
                }
                let port = port.parse::<u16>().map_err(|error| {
                    InternalTestReportError::InvalidUploadEndpoint(error.to_string())
                })?;
                Ok((host.to_owned(), port))
            },
        )?;
        Ok(Self {
            host,
            host_header: authority.to_owned(),
            port,
            path: path.to_owned(),
        })
    }
}

fn parse_http_response(response: &str) -> Result<(u16, String), InternalTestReportError> {
    let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((response, ""));
    let status_line = headers.lines().next().ok_or_else(|| {
        InternalTestReportError::InvalidUploadEndpoint("empty HTTP response".to_owned())
    })?;
    let mut parts = status_line.split_whitespace();
    let _http_version = parts.next();
    let status = parts
        .next()
        .ok_or_else(|| {
            InternalTestReportError::InvalidUploadEndpoint("missing HTTP status".to_owned())
        })?
        .parse::<u16>()
        .map_err(|error| InternalTestReportError::InvalidUploadEndpoint(error.to_string()))?;
    Ok((status, body.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_upload_endpoint() {
        let endpoint = HttpEndpoint::parse("http://upload.example.test:30080/upload").unwrap();

        assert_eq!(
            endpoint,
            HttpEndpoint {
                host: "upload.example.test".to_owned(),
                host_header: "upload.example.test:30080".to_owned(),
                port: 30080,
                path: "/upload".to_owned(),
            }
        );
    }
}
