// Copyright 2014 Dmitry "Divius" Tantsur <divius.inside@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
//

//! KRPC protocol bits as described in
//! [BEP 0005](http://www.bittorrent.org/beps/bep_0005.html).

use std::{collections,iter,fmt};
use std::old_path::BytesContainer;

use bencode::{self, Bencode, FromBencode, ToBencode};
use bencode::util::ByteString;
use std::num::FromPrimitive;
use std::num::ToPrimitive;
use num;

use super::super::base;
use super::super::utils;


// TODO(divius): actually validate it
static ID_BYTE_SIZE: usize = 20;

/// Type of payload dict.
pub type PayloadDict = bencode::DictMap;

/// Package payload in KRPC: either Query (request) or Response or Error.
#[derive(Clone)]
pub enum Payload {
    /// Request to a node.
    Query(PayloadDict),
    /// Response to request.
    Response(PayloadDict),
    /// Error: code and string message.
    Error(i64, String)
}

// TODO temp fn, cannot derive payload cause bencode utils::bytestring does not
impl fmt::Debug for Payload {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            &Payload::Query(..) => {
                try!(write!(fmt, "Query : ("));
            },
            &Payload::Response(..) => {
                try!(write!(fmt, "Response : ("));
            },
            &Payload::Error(ref code, ref mesg) => {
                try!(write!(fmt, "Error {:?} with message {:?}", code, mesg));
            }
        };
        match self {
          &Payload::Query(ref payload) | &Payload::Response(ref payload) => {
              for (k, v) in payload.iter() {
                  try!(write!(fmt, "\t({},{:?})", k, v));
              };
          },
          _ => {},
        };
        write!(fmt, ")")
    }
}

/// KRPC package.
#[derive(Clone)]
pub struct Package {
    /// Transaction ID generated by requester and passed back by responder.
    pub transaction_id: Vec<u8>,
    /// Package payload.
    pub payload: Payload,
    /// Sender node if known.
    ///
    /// Note that as per BEP 0005 it is stored in payload and thus is not set
    /// for errors.
    pub sender: Option<base::Node>
}


const QUERY: &'static str = "q";
const RESPONSE: &'static str = "r";
const ERROR: &'static str = "e";

const TYPE: &'static str = "y";
const TR_ID: &'static str = "tt";
const SENDER: &'static str = "id";


fn id_to_netbytes(id: &num::BigUint) -> Vec<u8> {
    assert!(id.bits() <= ID_BYTE_SIZE * 8);

    let mut id_c = id.clone();
    let mask: num::BigUint = FromPrimitive::from_u8(0xFF).unwrap();
    let mut result : Vec<u8> = iter::repeat(0).take(ID_BYTE_SIZE).collect();

    for i in result.iter_mut().rev() {
        let part: num::BigUint = &id_c & &mask;
        *i = part.to_u8().unwrap();
        id_c = id_c >> 8;
    }

    result
}

fn id_from_netbytes(bytes: &[u8]) -> num::BigUint {
    let mut result: num::BigUint = FromPrimitive::from_int(0).unwrap();
    let mut shift = 0;
    for i in bytes.iter().rev() {
        let val: num::BigUint = FromPrimitive::from_u8(*i).unwrap();
        result = result + (val << shift);
        shift += 8;
    }
    result
}

/// Helper function to build key for payload dict.
#[inline]
pub fn key(s: &str) -> ByteString {
    ByteString::from_str(s)
}

impl ToBencode for base::Node {
    fn to_bencode(&self) -> Bencode {
        let mut result = id_to_netbytes(&self.id);
        result.extend(utils::netaddr_to_netbytes(&self.address).into_iter());
        Bencode::ByteString(result)
    }
}

impl FromBencode for base::Node {
    fn from_bencode(b: &Bencode) -> Option<base::Node> {
        match *b {
            Bencode::ByteString(ref v) if v.len() == 26 => Some(base::Node {
                id: id_from_netbytes(&v[0..20]),
                address: utils::netaddr_from_netbytes(&v[20..26])
            }),
            _ => {
                debug!("{:?} is unexpected representation for a node", b);
                None
            }
        }
    }
}

fn dict_with_sender(dict: &PayloadDict, maybe_sender: &Option<base::Node>)
        -> bencode::Bencode {
    let mut d = dict.clone();
    if let Some(ref sender) = *maybe_sender {
        d.insert(key(SENDER), sender.to_bencode());
    }
    Bencode::Dict(d)
}

impl ToBencode for Package {
    fn to_bencode(&self) -> Bencode {
        let mut result = collections::BTreeMap::new();

        result.insert(key(TR_ID),
                      Bencode::ByteString(self.transaction_id.clone()));
        let (typ, payload) = match self.payload {
            Payload::Query(ref d) => (QUERY, dict_with_sender(d, &self.sender)),
            Payload::Response(ref d) => (RESPONSE, dict_with_sender(d, &self.sender)),
            Payload::Error(code, ref s) => {
                let l = vec![code.to_bencode(), s.to_bencode()];
                (ERROR, Bencode::List(l))
            }
        };
        result.insert(key(TYPE), typ.to_string().to_bencode());
        result.insert(key(typ), payload);

        Bencode::Dict(result)
    }
}

macro_rules! debug_and_return(
    ($( $arg:expr ),*) => ({
        debug!($( $arg ),*);
        return None;
    })
);

macro_rules! bytes_or_none(
    ($dict:ident, $key:expr, $msg:expr) => (
        match $dict.get(&key($key)) {
            Some(&Bencode::ByteString(ref val)) => val,
            _ => debug_and_return!($msg)
        }
    )
);

macro_rules! extract_sender(
    ($dict:ident, $ty:expr, $msg:expr) => ({
        let mut d = $dict.clone(); // TODO clone on btree map...
        if let Some(sender_be) = d.remove(&key(SENDER)) {
            if let Some(sender) = FromBencode::from_bencode(&sender_be) {
                ($ty(d), sender)
            }
            else {
                debug_and_return!("Cannot decode sender {:?}", sender_be);
            }
        }
        else {
            debug_and_return!($msg);
        }
    })
);

impl FromBencode for Package {
    fn from_bencode(b: &Bencode) -> Option<Package> {
        let dict = match *b {
            Bencode::Dict(ref d) => d,
            _ => debug_and_return!("Expected dict as top-level package, got {:?}", b)
        };

        let typ = bytes_or_none!(dict, TYPE, "No type");
        let payload_data = match dict.get(&ByteString::from_vec(typ.clone())) {
            Some(val) => val,
            None => debug_and_return!("No payload")
        };

        let (payload, sender) = match typ.container_as_str() {
            Some(ERROR) => match *payload_data {
                Bencode::List(ref v) => match v.as_slice() {
                    [Bencode::Number(code), Bencode::ByteString(ref msg)] => {
                        let str_msg = match msg.container_as_str() {
                            Some(s) => s,
                            None => {
                                debug!("Error message is not UTF8: {:?}", msg);
                                "Unknown error"
                            }
                        };
                        (Payload::Error(code, str_msg.to_string()), None)
                    },
                    _ => debug_and_return!(
                        "Error body of unknown structure {:?}", v)
                },
                _ => debug_and_return!("Error body of unexpected type: {:?}",
                                       payload_data)
            },
            Some(QUERY) => match *payload_data {
                Bencode::Dict(ref d) =>
                    extract_sender!(d, Payload::Query, "No sender ID in query"),
                _ => debug_and_return!("Query body of unexpected type: {:?}",
                                       payload_data)
            },
            Some(RESPONSE) => match *payload_data {
                Bencode::Dict(ref d) =>
                    extract_sender!(d, Payload::Response, "No sender ID in response"),
                _ => debug_and_return!("Response body of unexpected type: {:?}",
                                       payload_data)
            },
            Some(unknown) => debug_and_return!("Unexpected payload type {:?}",
                                               unknown),
            None => debug_and_return!("Not a UTF8 string: field y, value {:?}",
                                      typ)
        };

        let tt = bytes_or_none!(dict, TR_ID, "No transaction id");
        Some(Package {
            transaction_id: tt.clone(),
            payload: payload,
            sender: sender
        })
    }
}


#[cfg(test)]
mod test {
    use std::collections;
    use std::iter;

    use bencode::{self, Bencode, FromBencode, ToBencode};
    use bencode::util::ByteString;

    use super::super::super::base;
    use super::super::super::utils::test;

    use super::key;
    use super::PayloadDict;
    use super::Package;
    use super::Payload;


    const FAKE_TR_ID: [u8; 4] = [1, 2, 254, 255];

    fn new_package(payload: Payload) -> Package {
        Package {
            transaction_id: FAKE_TR_ID.to_vec(),
            sender: Some(test::new_node(42)),
            payload: payload
        }
    }

    fn common<'a>(b: &'a Bencode, typ: &str) -> &'a bencode::DictMap {
        match *b {
            Bencode::Dict(ref d) => {
                let tt_val = &d[key("tt")];
                match *tt_val {
                    Bencode::ByteString(ref v) => {
                        assert_eq!(vec![1, 2, 254, 255], *v);
                    },
                    _ => panic!("unexpected {:?}", tt_val)
                };

                let y_val = &d[key("y")];
                match *y_val {
                    Bencode::ByteString(ref v) => {
                        assert_eq!(typ.as_bytes(), v.as_slice());
                    },
                    _ => panic!("unexpected {:?}", y_val)
                };

                d
            },
            _ => panic!("unexpected {:?}", b)
        }
    }

    fn dict<'a>(b: &'a Bencode, typ: &str) -> &'a bencode::DictMap {
        let d = common(b, typ);

        let typ_val = &d[key(typ)];
        match *typ_val {
            Bencode::Dict(ref m) => m,
            _ => panic!("unexpected {:?}", typ_val)
        }
    }

    fn list<'a>(b: &'a Bencode, typ: &str) -> &'a bencode::ListVec {
        let d = common(b, typ);

        let typ_val = &d[key(typ)];
        match *typ_val {
            Bencode::List(ref l) => l,
            _ => panic!("unexpected {:?}", typ_val)
        }
    }

    #[test]
    fn test_error_to_bencode() {
        let p = new_package(Payload::Error(10, "error".to_string()));
        let enc = p.to_bencode();
        let l = list(&enc, "e");
        assert_eq!(vec![Bencode::Number(10),
                        "error".to_string().to_bencode()],
                   *l);
    }

    #[test]
    fn test_error_to_from_bencode() {
        let p = new_package(Payload::Error(10, "error".to_string()));
        let enc = p.to_bencode();
        let p2: Package = FromBencode::from_bencode(&enc).unwrap();
        assert_eq!(FAKE_TR_ID, p2.transaction_id.as_slice());
        assert!(p2.sender.is_none());
        if let Payload::Error(code, msg) = p2.payload {
            assert_eq!(10, code);
            assert_eq!("error", msg.as_slice());
        }
        else {
            panic!("Expected Error, got {:?}", p2.payload);
        }
    }

    #[test]
    fn test_query_to_bencode() {
        let payload: PayloadDict = collections::BTreeMap::new();
        let p = new_package(Payload::Query(payload.clone()));
        let enc = p.to_bencode();
        let d = dict(&enc, "q");
        assert_eq!(1, d.len());
        assert!(d.contains_key(&key("id")));
    }

    #[test]
    fn test_query_to_from_bencode() {
        let mut payload: PayloadDict = collections::BTreeMap::new();
        payload.insert(key("test"), "ok".to_string().to_bencode());
        let p = new_package(Payload::Query(payload));
        let enc = p.to_bencode();
        let p2: Package = FromBencode::from_bencode(&enc).unwrap();
        assert_eq!(FAKE_TR_ID, p2.transaction_id.as_slice());
        assert_eq!(test::usize_to_id(42), p2.sender.unwrap().id);
        if let Payload::Query(d) = p2.payload {
            assert_eq!(1, d.len());
            assert_eq!(Bencode::ByteString(vec![111, 107]),
                       d[key("test")]);
        }
        else {
            panic!("Expected Query, got {:?}", p2.payload);
        }
    }

    #[test]
    fn test_response_to_bencode() {
        let payload: PayloadDict = collections::BTreeMap::new();
        let p = new_package(Payload::Response(payload));
        let enc = p.to_bencode();
        let d = dict(&enc, "r");
        assert_eq!(1, d.len());
        assert!(d.contains_key(&key("id")));
    }

    #[test]
    fn test_response_to_from_bencode() {
        let mut payload: PayloadDict = collections::BTreeMap::new();
        payload.insert(key("test"), "ok".to_string().to_bencode());
        let p = new_package(Payload::Response(payload));
        let enc = p.to_bencode();
        let p2: Package = FromBencode::from_bencode(&enc).unwrap();
        assert_eq!(FAKE_TR_ID, p2.transaction_id.as_slice());
        assert_eq!(test::usize_to_id(42), p2.sender.unwrap().id);
        if let Payload::Response(d) = p2.payload {
            assert_eq!(1, d.len());
            assert_eq!(Bencode::ByteString(vec![111, 107]),
                       d[key("test")]);
        }
        else {
            panic!("Expected Response, got {:?}", p2.payload);
        }
    }

    #[test]
    fn test_id_to_netbytes() {
        let id = test::usize_to_id(0x0A0B0C0D);
        let b = super::id_to_netbytes(&id);
        let mut expected : Vec<u8> = iter::repeat(0u8).take(16).collect();
        expected.push_all(&[0x0A, 0x0b, 0x0C, 0x0D]);
        assert_eq!(expected, b);
    }

    #[test]
    fn test_id_from_netbytes() {
        let mut bytes : Vec<u8> = iter::repeat(0u8).take(16).collect();
        bytes.push_all(&[0x0A, 0x0b, 0x0C, 0x0D]);
        let expected = test::usize_to_id(0x0A0B0C0D);
        let id = super::id_from_netbytes(bytes.as_slice());
        assert_eq!(expected, id);
    }

    #[test]
    fn test_node_to_bencode() {
        let n = test::new_node(42);
        let enc = n.to_bencode();
        let mut expected : Vec<u8> = iter::repeat(0u8).take(19).collect();
        expected.push_all(&[42, 127, 0, 0, 1, 31, 72]);
        assert_eq!(Bencode::ByteString(expected), enc);
    }

    #[test]
    fn test_node_from_bencode() {
        let mut b : Vec<u8> = iter::repeat(0u8).take(19).collect();
        b.push_all(&[42, 127, 0, 0, 1, 0, 80]);
        let n: base::Node =
            FromBencode::from_bencode(&Bencode::ByteString(b)).unwrap();
        assert_eq!(n.id, test::usize_to_id(42));
        assert_eq!(n.address.to_string().as_slice(), "127.0.0.1:80");
    }

    #[test]
    fn test_node_from_bencode_none() {
        let n: Option<base::Node> =
            FromBencode::from_bencode(&Bencode::Number(42));
        assert!(n.is_none());
    }

    #[test]
    fn test_node_to_from_bencode() {
        let n = test::new_node(42);
        let enc = n.to_bencode();
        let n2: base::Node = FromBencode::from_bencode(&enc).unwrap();
        assert_eq!(n.id, n2.id);
        assert_eq!(n.address, n2.address);
    }
}
