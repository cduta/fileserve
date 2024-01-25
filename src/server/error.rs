use std::{io, string, fmt, error, num};

pub enum ServerError {
  TransportError(io::Error),
  ConvertError(string::FromUtf8Error),
  ParseIntError(num::ParseIntError),
  HTTPParseError(String)
}

impl fmt::Debug for ServerError {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::TransportError(e) => write!(f, "TransportError {}",  e),
      Self::ConvertError(e)   => write!(f, "ConvertError {}"  ,  e),
      Self::ParseIntError(e)  => write!(f, "ParseIntError {}" ,  e),
      Self::HTTPParseError(e) => write!(f, "HTTPParseError {}",  e)
    }
  }
}

impl fmt::Display for ServerError {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::TransportError(e) => write!(f, "TransportError {}",  e),
      Self::ConvertError(e)   => write!(f, "ConvertError {}"  ,  e),
      Self::ParseIntError(e)  => write!(f, "ParseIntError {}" ,  e),
      Self::HTTPParseError(e) => write!(f, "HTTPParseError {}",  e)
    }
  }
}

impl error::Error for ServerError {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      Self::TransportError(ref e) => Some(e),
      Self::ConvertError(ref e)   => Some(e),
      Self::ParseIntError(ref e)  => Some(e),
      Self::HTTPParseError(_)     => None
    }
  }
}

impl From<io::Error> for ServerError {
  fn from(e: io::Error) -> Self { Self::TransportError(e) }
}

impl From<string::FromUtf8Error> for ServerError {
  fn from(e: string::FromUtf8Error) -> Self { Self::ConvertError(e) }
}

impl From<num::ParseIntError> for ServerError {
  fn from(e: num::ParseIntError) -> Self { Self::ParseIntError(e) }
}