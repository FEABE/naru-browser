//! DevTools request encoding and borrowed response/event envelopes.
//!
//! Header method/session strings must be directly borrowable JSON strings.
//! Escaped spellings are rejected; ordinary CDP method names and session IDs
//! need no decoding. Result/error/params bodies retain their exact source bytes.

use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use std::fmt;

pub fn serialize_request(id: i32, method: &str, params: &Value) -> Option<Vec<u8>> {
    serialize_session_request(id, method, params, None)
}

pub fn serialize_session_request(
    id: i32,
    method: &str,
    params: &Value,
    session: Option<&str>,
) -> Option<Vec<u8>> {
    if id <= 0
        || !valid_method(method)
        || !params.is_object()
        || session.is_some_and(|value| !valid_session(value))
    {
        return None;
    }
    #[derive(Serialize)]
    struct Request<'a> {
        id: i32,
        method: &'a str,
        params: &'a Value,
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session: Option<&'a str>,
    }
    serde_json::to_vec(&Request {
        id,
        method,
        params,
        session,
    })
    .ok()
}

fn valid_method(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_')
}

fn valid_session(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

pub(crate) fn incoming_message(bytes: &[u8]) -> Option<Incoming<'_>> {
    serde_json::from_slice(bytes).ok()
}

#[derive(Debug)]
pub(crate) enum Incoming<'a> {
    Response {
        id: i32,
        session: Option<&'a str>,
        success: bool,
        payload: &'a [u8],
    },
    Event {
        session: Option<&'a str>,
        method: &'a str,
        params: &'a [u8],
    },
}

#[derive(Clone, Copy)]
enum Field {
    Id,
    Method,
    SessionId,
    Result,
    Error,
    Params,
    Other,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor;
        impl Visitor<'_> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a DevTools envelope field")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Field, E> {
                Ok(match value {
                    "id" => Field::Id,
                    "method" => Field::Method,
                    "sessionId" => Field::SessionId,
                    "result" => Field::Result,
                    "error" => Field::Error,
                    "params" => Field::Params,
                    _ => Field::Other,
                })
            }
        }
        deserializer.deserialize_identifier(FieldVisitor)
    }
}

impl<'de> Deserialize<'de> for Incoming<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ResponseVisitor;
        impl<'de> Visitor<'de> for ResponseVisitor {
            type Value = Incoming<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a DevTools response or event object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Incoming<'de>, M::Error> {
                let mut id = None;
                let mut seen = 0_u8;
                let mut method = None;
                let mut session = None;
                let mut result = false;
                let mut error = false;
                let mut payload: Option<&'de RawValue> = None;
                let mut params: Option<&'de RawValue> = None;
                while let Some(field) = map.next_key::<Field>()? {
                    let bit = match field {
                        Field::Id => 1,
                        Field::Method => 2,
                        Field::SessionId => 4,
                        Field::Result => 8,
                        Field::Error => 16,
                        Field::Params => 32,
                        Field::Other => 0,
                    };
                    if seen & bit != 0 {
                        return Err(de::Error::custom("duplicate DevTools envelope field"));
                    }
                    seen |= bit;
                    match field {
                        Field::Id => id = Some(map.next_value::<i32>()?),
                        Field::Method => {
                            let value = map.next_value::<&'de str>()?;
                            if !valid_method(value) {
                                return Err(de::Error::custom("invalid DevTools method"));
                            }
                            method = Some(value);
                        }
                        Field::SessionId => {
                            let value = map.next_value::<&'de str>()?;
                            if !valid_session(value) {
                                return Err(de::Error::custom("invalid DevTools session id"));
                            }
                            session = Some(value);
                        }
                        Field::Result | Field::Error | Field::Params => {
                            // RawValue validates JSON while borrowing its exact
                            // source representation; do not parse a Value tree or
                            // reserialize the body delivered to the consumer.
                            let body = map.next_value::<&'de RawValue>()?;
                            if !body.get().starts_with('{') {
                                return Err(de::Error::custom("DevTools payload is not an object"));
                            }
                            if matches!(field, Field::Params) {
                                params = Some(body);
                            } else {
                                payload = Some(body);
                                result = matches!(field, Field::Result) || result;
                                error = matches!(field, Field::Error) || error;
                            }
                        }
                        Field::Other => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                if let Some(method) = method {
                    if id.is_some() || result || error {
                        return Err(de::Error::custom("mixed DevTools event and response"));
                    }
                    return Ok(Incoming::Event {
                        session,
                        method,
                        params: params.map_or(b"{}", |value| value.get().as_bytes()),
                    });
                }
                let Some(id) = id.filter(|value| *value > 0) else {
                    return Err(de::Error::custom("missing or invalid DevTools response id"));
                };
                if params.is_some() || result == error {
                    return Err(de::Error::custom("invalid DevTools response envelope"));
                }
                let Some(payload) = payload else {
                    return Err(de::Error::custom("missing DevTools response payload"));
                };
                Ok(Incoming::Response {
                    id,
                    session,
                    success: result,
                    payload: payload.get().as_bytes(),
                })
            }
        }
        deserializer.deserialize_map(ResponseVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_encoding_preserves_data_and_rejects_invalid_envelopes() {
        let params = json!({"text":"quote \" and newline\n한글","nested":{"value":true}});
        let encoded = serialize_request(17, "Input.insertText", &params).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded).unwrap(),
            json!({"id":17,"method":"Input.insertText","params":params})
        );
        assert!(serialize_request(0, "Method", &json!({})).is_none());
        assert!(serialize_request(-1, "Method", &json!({})).is_none());
        assert!(serialize_request(1, "Method", &json!([])).is_none());
        assert!(serialize_request(1, "Method", &Value::Null).is_none());
        let encoded =
            serialize_session_request(2, "Runtime.evaluate", &json!({}), Some("child-1")).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded).unwrap(),
            json!({"id":2,"method":"Runtime.evaluate","params":{},"sessionId":"child-1"})
        );
        for method in ["", "Input.bad name", "Input.문자", &"a".repeat(129)] {
            assert!(serialize_request(1, method, &json!({})).is_none());
        }
        for session in ["", "line\nbreak", &"x".repeat(257)] {
            assert!(serialize_session_request(1, "Method", &json!({}), Some(session)).is_none());
        }
    }

    #[test]
    fn response_and_event_envelopes_preserve_borrowed_bodies_and_reject_mixed_headers() {
        let borrowed = |input: &[u8], part: &[u8]| {
            let offset = input
                .windows(part.len())
                .position(|window| window == part)
                .unwrap();
            assert_eq!(
                part.as_ptr(),
                input[offset..].as_ptr(),
                "field must borrow the input slice"
            );
        };
        for (valid, expected_id, expected_session, expected_success, body) in [
            (
                r#"{"id":1,"result":{"nested":[true,null,{"value":"text"}]}}"#,
                1,
                None,
                true,
                r#"{"nested":[true,null,{"value":"text"}]}"#,
            ),
            (
                r#"{"error":{"code":-32000,"message":"failed"},"id":2147483647,"sessionId":"child-1"}"#,
                i32::MAX,
                Some("child-1"),
                false,
                r#"{"code":-32000,"message":"failed"}"#,
            ),
            (
                r#"{"id":1,"result":{},"future":[],"future":null}"#,
                1,
                None,
                true,
                "{}",
            ),
            (
                r#"{ "result" : { "n" : 1.00, "s":"\u0061" }, "id" : 7 }"#,
                7,
                None,
                true,
                r#"{ "n" : 1.00, "s":"\u0061" }"#,
            ),
        ] {
            let bytes = valid.as_bytes();
            let Incoming::Response {
                id,
                session,
                success,
                payload,
            } = incoming_message(bytes).unwrap()
            else {
                panic!("expected response: {valid}");
            };
            assert_eq!(
                (id, session, success),
                (expected_id, expected_session, expected_success),
                "{valid}"
            );
            assert_eq!(payload, body.as_bytes(), "{valid}");
            borrowed(bytes, payload);
            if let Some(session) = session {
                borrowed(bytes, session.as_bytes());
            }
        }
        for (valid, expected_session, expected_method, body) in [
            (
                r#"{"method":"Target.attachedToTarget","params":{ "sessionId":"nested", "x":1.00 }}"#,
                None,
                "Target.attachedToTarget",
                Some(r#"{ "sessionId":"nested", "x":1.00 }"#),
            ),
            (
                r#"{"sessionId":"child-1","params":{},"method":"Runtime.context_2"}"#,
                Some("child-1"),
                "Runtime.context_2",
                Some("{}"),
            ),
            (
                r#"{"method":"Page.loadEventFired"}"#,
                None,
                "Page.loadEventFired",
                None,
            ),
        ] {
            let bytes = valid.as_bytes();
            let Incoming::Event {
                session,
                method,
                params,
            } = incoming_message(bytes).unwrap()
            else {
                panic!("expected event: {valid}");
            };
            assert_eq!((session, method), (expected_session, expected_method));
            assert_eq!(params, body.unwrap_or("{}").as_bytes());
            borrowed(bytes, method.as_bytes());
            if let Some(session) = session {
                borrowed(bytes, session.as_bytes());
            }
            if body.is_some() {
                borrowed(bytes, params);
            }
        }
        for invalid in [
            "[]",
            "null",
            "{}",
            r#"{"id":1}"#,
            r#"{"result":{}}"#,
            r#"{"id":0,"result":{}}"#,
            r#"{"id":-1,"result":{}}"#,
            r#"{"id":2147483648,"result":{}}"#,
            r#"{"id":1.0,"result":{}}"#,
            r#"{"id":"1","result":{}}"#,
            r#"{"id":null,"result":{}}"#,
            r#"{"id":1,"result":null}"#,
            r#"{"id":1,"result":[]}"#,
            r#"{"id":1,"error":"failure"}"#,
            r#"{"id":1,"result":{},"error":{}}"#,
            r#"{"id":1,"result":{},"method":null}"#,
            r#"{"id":1,"result":{},"method":"Page.event"}"#,
            r#"{"id":null,"method":"Page.event"}"#,
            r#"{"method":"Page.event","result":{}}"#,
            r#"{"method":"Page.event","error":{}}"#,
            r#"{"id":1,"result":{},"params":{}}"#,
            r#"{"method":"Page.event","params":null}"#,
            r#"{"method":"Page.event","params":[]}"#,
            r#"{"method":"Page.event","params":{},"params":{}}"#,
            r#"{"method":""}"#,
            r#"{"method":"Page.bad name"}"#,
            r#"{"method":"Page.\u0065vent"}"#,
            r#"{"id":1,"result":{},"sessionId":"\u0063hild"}"#,
            r#"{"id":1,"id":2,"result":{}}"#,
            r#"{"id":1,"i\u0064":2,"result":{}}"#,
            r#"{"id":1,"result":{},"result":{}}"#,
            r#"{"id":1,"error":{},"error":{}}"#,
            r#"{"id":1,"method":"event","method":null,"result":{}}"#,
            r#"{"id":1,"sessionId":null,"sessionId":null,"result":{}}"#,
            r#"{"id":1,"result":{}} {}"#,
            r#"{"id":1,"result":{"bad":[1,]}}"#,
        ] {
            assert!(incoming_message(invalid.as_bytes()).is_none(), "{invalid}");
        }
        for session in ["null", "1", "true", r#""""#, r#""line\nbreak""#, "{}", "[]"] {
            let response = format!(r#"{{"id":1,"result":{{}},"sessionId":{session}}}"#);
            assert!(
                incoming_message(response.as_bytes()).is_none(),
                "{response}"
            );
        }
        for message in [
            format!(r#"{{"method":"{}"}}"#, "m".repeat(129)),
            format!(
                r#"{{"id":1,"result":{{}},"sessionId":"{}"}}"#,
                "s".repeat(257)
            ),
            format!(r#"{{"method":"Page.event","sessionId":"a{}b"}}"#, '\u{85}'),
        ] {
            assert!(incoming_message(message.as_bytes()).is_none());
        }
    }
}
