use std::collections::HashMap;
use std::fmt::Display;
use std::fs::File;
use std::net::TcpListener;
use std::{fs, io};
use std::io::prelude::*;
use std::net::TcpStream;
use std::os::unix::ffi::OsStrExt;

mod threadpool;
mod error;

use threadpool::ThreadPool;
use error::ServerError;

enum HTTPRequestType { GET, POST }

impl Display for HTTPRequestType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      HTTPRequestType::GET  => write!(f, "GET"),
      HTTPRequestType::POST => write!(f, "POST")
    }
  }
}

impl TryFrom<&str> for HTTPRequestType {
    type Error = ServerError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
      match value {
        "GET"  => Ok(HTTPRequestType::GET),
        "POST" => Ok(HTTPRequestType::POST),
        _      => Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Cannot convert {value} to HTTPRequestType")))
      }
    }
}

type HTTPSettings = HashMap<String,String>;

struct Request {
  r_type : HTTPRequestType,
  url    : String,
  version: String,
  content: HTTPSettings
}

impl Request {
  fn parse_header(req_string: String) -> Result<Self,ServerError> {
    let lines = req_string
      .split("\r\n")
      .collect::<Vec<&str>>();

    if lines.is_empty() { return Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Invalid request string ({req_string})"))) }

    let header = lines[0].split(' ').collect::<Vec<&str>>();

    if header.len() != 3 { return Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Malformed header ({})", lines[0]))) }

    let content = lines
      .iter()
      .skip(1)
      .fold(HashMap::new(), |mut acc, s| {
        if let Some((name, value)) = s.split_once(": ") {
          if let Some(prev) = acc.insert(name.to_string(), value.trim().to_string()) {
            println!("HTTP Parse Warning: Duplicate entry {name} replaces {prev} with {value}")
          }
        } else {
          println!("HTTP Parse Warning: Malformed header or body ({s})");
        }
        acc
      });

    Ok(Self {
      r_type : HTTPRequestType::try_from(header[0])?,
      url    : header[1].to_string(),
      version: header[2].to_string(),
      content
    })
  }
}

impl Display for Request {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "Request Type: {}\nVersion: {}\nURL: {}\nSettings: {:?}", self.r_type, self.version, self.url, self.content)
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
  const BUFFER_SIZE: usize = 8096;
  let mut buffer = [0; BUFFER_SIZE];
  const HEADER_END: [u8; 4] = [13,10,13,10];

  let mut header_string: String = String::new();

  // Get Request
  stream.read(&mut buffer)?;

  let mut i = 4;
  for window in buffer.as_slice().windows(4) {
    if  window[0] == HEADER_END[0]
     && window[1] == HEADER_END[1]
     && window[2] == HEADER_END[2]
     && window[3] == HEADER_END[3] {
      break;
    }
    header_string.push(char::from(window[0]));
    i+=1;
  }

  let mut body: Vec<u8> = buffer[i..].to_vec();

  println!("### BEGIN REQUEST ###");
  println!("{header_string}");
  println!("\nBody\n----\n{}", String::from_utf8_lossy(&body));
  println!("### END REQUEST ###");

  // Parse Header (Tokenize: "<GET|...> <URL> <HTTP/\d+.\d+>\n(<field>:<value>\n)+")
  let header = Request::parse_header(header_string)?;

  let ok        = "HTTP/1.1 200 OK";
  let not_found = "HTTP/1.1 404 NOT FOUND";

  // Evaluate header
  let (status_line, contents) =
    if header.url.contains("..") {
      println!("Server Error: Indirection in path forbidden");
      (not_found, "Woops".as_bytes().to_vec())
    } else {
      let path = header.url.chars().skip(1).collect::<String>();
      match header.r_type {
        HTTPRequestType::GET => {
          if header.url.ends_with("/") {
            (ok, fs::read_to_string("files.html")?.replace("{{Entries}}", String::from_utf8(dir(path)?)?.as_str()).as_bytes().to_vec())
          } else if path.starts_with("static/icons") {
            (ok, fs::read(path)?)
          } else {
            (ok, fs::read(format!("files/{path}"))?)
          }
        },
        HTTPRequestType::POST => {
          if header.content.contains_key("Action") || header.content.contains_key("Content-Type") {
            if let Some(action) = header.content.get("Action") {
              match action.as_str() {
                "create_file" => {
                  let relative_path = ["files",header.url.as_str()].concat();
                  fs::create_dir(relative_path.clone())?;
                  (ok, ["Directory ", relative_path.as_str(), " created..."].concat().as_bytes().to_vec())
                },
                _ => {
                  println!("Server Error: Invalid Action `{action}`");
                  (not_found, "Woops".as_bytes().to_vec())
                }
              }
            } else if let (Some(_content_type), Some(content_length)) = (header.content.get("Content-Type"),header.content.get("Content-Length")) {

              let from_utf8_lossy = &String::from_utf8_lossy(&body);
              let content_header = from_utf8_lossy.splitn(3, "\r\n").collect::<Vec<&str>>();
              let _content_boundary = content_header[0];
              let content_dispositions = content_header[1]
               .split_once(": ")
               .unwrap().1
               .split("; ")
               .fold(HashMap::new(), |mut acc, e| {
                if let Some((key,value)) = e.split_once("=") {
                  acc.insert(key, &value[1..value.len()-1]);
                }
                acc
               });
              let _content_type = content_header[2].split_once(": ").unwrap().1;
              let mut file = File::create(["files/",path.as_str(),content_dispositions.get("filename").unwrap()].concat())?;

              file.write(&body)?;
              let mut total_bytes_read = body.len();
              while total_bytes_read < content_length.parse::<usize>()? {
                let bytes_read = stream.read(&mut buffer)?;
                total_bytes_read += bytes_read;
                body = buffer[0..bytes_read].to_vec();
                file.write(&body)?;
                println!("\nMore Body\n---------\n{}", String::from_utf8_lossy(&body));
              }
              (not_found, "Woops".as_bytes().to_vec())
            } else {
              println!("Server Error: POST is neither an Action nor has a Content-Type");
              (not_found, "Woops".as_bytes().to_vec())
            }
          } else {
            println!("Server Error: POST is neither an Action nor has a Content-Type");
            (not_found, "Woops".as_bytes().to_vec())
          }
        }
      }
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

  if let Err(e) = fs::read_dir("files/") {
    if e.kind() == io::ErrorKind::NotFound {
      if let Err(e) = fs::create_dir("files/") {
        println!("Initialize Server Error: {e}"); rc = 1;
      }
    } else if e.kind() != io::ErrorKind::AlreadyExists {
      println!("Initialize Server Error: {e}"); rc = 2;
    }
  }

  if rc == 0 {
    match TcpListener::bind("192.168.178.43:8000") {
      Ok(listener) => {
        match ThreadPool::new(16) {
          Ok(pool) => listen(listener, pool),
          Err(e)   => { println!("Create Thread Pool Error: {e}"); rc = 3; }
        }
      },
      Err(e) => { println!("Could not start server: {e}"); rc = 4; }
    }
    println!("Shutting down...OK");
  }
  rc
}