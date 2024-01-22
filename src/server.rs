use std::collections::HashMap;
use std::fmt::Display;
use std::net::TcpListener;
use std::fs;
use std::io::prelude::*;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

mod threadpool;
mod error;

use threadpool::ThreadPool;
use error::ServerError;

enum HTTPRequestType { GET }

impl Display for HTTPRequestType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      HTTPRequestType::GET  => write!(f, "GET")
    }
  }
}

impl TryFrom<&str> for HTTPRequestType {
    type Error = ServerError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
      match value {
        "GET"  => Ok(HTTPRequestType::GET),
        _      => Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Cannot convert {value} to HTTPRequestType")))
      }
    }
}

type HTTPSettings = HashMap<String,String>;

struct Request {
  r_type  : HTTPRequestType,
  url     : String,
  version : String,
  settings: HTTPSettings
}

impl Request {
  fn parse(req_string: String) -> Result<Self,ServerError> {
    let lines = req_string
      .split("\r\n")
      .filter(|s| !s.chars().all(|c| c == '\0'))
      .collect::<Vec<&str>>();

    if lines.is_empty() { return Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Invalid request string ({req_string})"))) }

    let header = lines[0].split(' ').collect::<Vec<&str>>();

    if header.len() != 3 { return Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Malformed header ({})", lines[0]))) }

    let settings = lines
      .iter()
      .skip(1)
      .fold(HashMap::new(), |mut acc, s| {
        if let Some((name, value)) = s.split_once(':') {
          if let Some(prev) = acc.insert(name.to_string(), value.to_string()) {
            println!("HTTP Parse Warning: Duplicate entry {name} replaces {prev} with {name}")
          }
        } else {
          println!("HTTP Parse Error: Malformed settings string ({s})");
        }
        acc
      });

    Ok(Self {
      r_type : HTTPRequestType::try_from(header[0])?,
      url    : header[1].to_string(),
      version: header[2].to_string(),
      settings
    })
  }
}

impl Display for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Request Type: {}\nVersion: {}\nURL: {}\nSettings: {:?}", self.r_type, self.version, self.url, self.settings)
    }
}

fn serve(mut stream: TcpStream) -> Result<(), ServerError> {
  let mut buffer = [0; 8096];

  // Get Request
  stream.read(&mut buffer)?;

  // Parse Request (Tokenize: "<GET|...> <URL> <HTTP/\d+.\d+>\n(<field>:<value>\n)+")
  let request = Request::parse(String::from_utf8(buffer.to_vec())?)?;

  println!("### BEGIN REQUEST ###");
  println!("{request}");
  println!("### END REQUEST ###");

  // Evaluate request
  let (status_line, filename) = match request.url.as_str() {
    "/"      => {("HTTP/1.1 200 OK", "hello.html")},
    "/sleep" => {thread::sleep(Duration::from_secs(5)); ("HTTP/1.1 200 OK", "hello.html")},
    _        => {("HTTP/1.1 404 NOT FOUND", "404.html")}
  };

  // Build content for response
  let contents = fs::read_to_string(filename)?;

  // Build response
  let response = format!(
    "{}\r\nContent-Length: {}\r\n\r\n{}",
    status_line,
    contents.len(),
    contents
  );

  // Respond
  stream.write_all(response.as_bytes())?;
  stream.flush()?;

  // Done
  Ok(())
}

fn listen(listener: TcpListener, pool: ThreadPool) {
  for stream in listener.incoming() {
    match stream {
      Ok(stream)  => pool.execute(move || if let Err(e) = serve(stream) { println!("Request failed: {e}") }),
      Err(e)      => println!("{e}")
    }
  }
}

pub fn run() -> i32 {
  let mut rc = 0;

  match TcpListener::bind("127.0.0.1:8000") {
    Ok(listener) => {
      match ThreadPool::new(16) {
        Ok(pool) => listen(listener, pool),
        Err(e)   => { println!("Create Thread Pool Error: {e}"); rc = 1; }
      }
    },
    Err(e) => { println!("Could not start server: {e}"); rc = 2; }
  }
  println!("Shutting down...OK");
  rc
}