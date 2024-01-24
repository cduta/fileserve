use std::collections::HashMap;
use std::fmt::Display;
use std::net::TcpListener;
use std::fs;
use std::io::prelude::*;
use std::net::TcpStream;
use std::os::unix::ffi::OsStrExt;

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

fn compile_response(status_line: &str, contents: Vec<u8>) -> Vec<u8> {
  [status_line.as_bytes(),
   "\r\nContent-Length:".as_bytes(),
   contents.len().to_string().as_bytes(),
   "\r\n\r\n".as_bytes(),
   &contents].concat()
}

fn dir(relative_path: String) -> Result<Vec<u8>, ServerError>  {
  let absolute_path = format!("files/{relative_path}");
  let (mut dirs, mut files): (Vec<Vec<u8>>,Vec<Vec<u8>>) = fs::read_dir(&absolute_path)?.fold(
    (Vec::new(),Vec::new()),
    |mut acc, r_entry| {
      match r_entry {
        Ok(entry) => {
          match entry.metadata() {
            Ok(md) if md.is_dir()  => acc.0.push(entry.file_name().as_bytes().to_vec()),
            Ok(md) if md.is_file() => acc.1.push(entry.file_name().as_bytes().to_vec()),
            Ok(_)                  => println!("List Dir Error: {:?} is neither file nor dir", entry.file_name()),
            Err(e)                 => println!("List Dir Error: {}", e.to_string())
          } acc
        },
        Err(e)    => { println!("List Dir Error: {}", e.to_string()); acc }
      }
    }
  );

  dirs.sort();
  dirs = dirs.into_iter().map(|s| ["<a href=\"".as_bytes().to_vec(),s.clone(),"/\"><button class=\"btnLink\">".as_bytes().to_vec(),s,"/</button></a>".as_bytes().to_vec()].concat()).collect();
  files.sort();
  files = files.into_iter().map(|s| ["<button class=\"btnLink\" onclick=\"javascript:download('".as_bytes().to_vec(),s.clone(),"')\" onmouseenter=\"javascript:show_preview('".as_bytes().to_vec(),s.clone(),"');\" onmousedown=\"javascript:show_preview('".as_bytes().to_vec(),s.clone(),"');\"') onmouseleave=\"javascript:hide_preview();\" onmouseout=\"javascript:hide_preview();\" onmouseup=\"javascript:hide_preview();\">".as_bytes().to_vec(),s,"</button>".as_bytes().to_vec()].concat() ).collect();
  dirs.append(files.as_mut());
  Ok(dirs.into_iter().fold(Vec::new(), |mut acc: Vec<u8>, mut entry| { acc.append(&mut entry); acc.append("<br>".as_bytes().to_vec().as_mut()); acc }))
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

  let ok        = "HTTP/1.1 200 OK";
  let not_found = "HTTP/1.1 404 NOT FOUND";

  // Evaluate request
  let (status_line, contents) = match request.url.chars().skip(1).collect::<String>() {
    path if !path.contains("..")
         && request.url.ends_with("/") => {(ok, fs::read_to_string("files.html")?.replace("{{Entries}}", String::from_utf8(dir(path)?)?.as_str()).as_bytes().to_vec())},
    path if !path.contains("..")       => {
            if path.starts_with("static/icons") {
              (ok, fs::read(path)?)
            } else {
              (ok, fs::read(format!("files/{path}"))?)
            }
          },
    _ => {(not_found, "Woops".as_bytes().to_vec())}
  };

  // Respond
  stream.write_all(compile_response(status_line, contents).as_slice())?;
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

  match TcpListener::bind("192.168.178.43:8000") {
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