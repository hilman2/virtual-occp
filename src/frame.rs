//! OCPP-J frame envelope: Call, CallResult, CallError.
//!
//! The wire format is a JSON array:
//!   [2, "<MessageId>", "<Action>", {payload}]   -> Call
//!   [3, "<MessageId>", {payload}]               -> CallResult
//!   [4, "<MessageId>", "<ErrorCode>", "<Description>", {ErrorDetails}] -> CallError

use serde_json::Value;

pub const MSG_CALL: u8 = 2;
pub const MSG_RESULT: u8 = 3;
pub const MSG_ERROR: u8 = 4;

#[derive(Debug, Clone)]
pub enum Frame {
    Call {
        id: String,
        action: String,
        payload: Value,
    },
    Result {
        id: String,
        payload: Value,
    },
    Error {
        id: String,
        code: String,
        description: String,
        details: Value,
    },
}

impl Frame {
    pub fn to_wire(&self) -> String {
        match self {
            Frame::Call {
                id,
                action,
                payload,
            } => serde_json::to_string(&(MSG_CALL, id, action, payload)).unwrap(),
            Frame::Result { id, payload } => {
                serde_json::to_string(&(MSG_RESULT, id, payload)).unwrap()
            }
            Frame::Error {
                id,
                code,
                description,
                details,
            } => serde_json::to_string(&(MSG_ERROR, id, code, description, details)).unwrap(),
        }
    }

    pub fn from_wire(s: &str) -> anyhow::Result<Self> {
        let arr: Vec<Value> = serde_json::from_str(s)?;
        let kind = arr
            .first()
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Frame without type indicator: {s}"))?;
        match kind as u8 {
            MSG_CALL => {
                if arr.len() != 4 {
                    anyhow::bail!("Call expects 4 elements, got {}", arr.len());
                }
                Ok(Frame::Call {
                    id: arr[1].as_str().unwrap_or_default().to_string(),
                    action: arr[2].as_str().unwrap_or_default().to_string(),
                    payload: arr[3].clone(),
                })
            }
            MSG_RESULT => {
                if arr.len() != 3 {
                    anyhow::bail!("CallResult expects 3 elements, got {}", arr.len());
                }
                Ok(Frame::Result {
                    id: arr[1].as_str().unwrap_or_default().to_string(),
                    payload: arr[2].clone(),
                })
            }
            MSG_ERROR => {
                if arr.len() < 4 {
                    anyhow::bail!("CallError expects >=4 elements, got {}", arr.len());
                }
                Ok(Frame::Error {
                    id: arr[1].as_str().unwrap_or_default().to_string(),
                    code: arr[2].as_str().unwrap_or_default().to_string(),
                    description: arr[3].as_str().unwrap_or_default().to_string(),
                    details: arr
                        .get(4)
                        .cloned()
                        .unwrap_or(Value::Object(Default::default())),
                })
            }
            other => anyhow::bail!("Unknown frame type: {other}"),
        }
    }

    pub fn new_call(action: impl Into<String>, payload: Value) -> (String, Self) {
        let id = uuid::Uuid::new_v4().to_string();
        (
            id.clone(),
            Frame::Call {
                id,
                action: action.into(),
                payload,
            },
        )
    }

    pub fn new_result(id: String, payload: Value) -> Self {
        Frame::Result { id, payload }
    }

    pub fn new_error(id: String, code: &str, description: &str) -> Self {
        Frame::Error {
            id,
            code: code.into(),
            description: description.into(),
            details: Value::Object(Default::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn call_roundtrip() {
        let f = Frame::Call {
            id: "abc-1".into(),
            action: "BootNotification".into(),
            payload: json!({"chargePointVendor":"A","chargePointModel":"B"}),
        };
        let wire = f.to_wire();
        assert!(wire.starts_with("[2,"));
        let parsed = Frame::from_wire(&wire).unwrap();
        match parsed {
            Frame::Call {
                id,
                action,
                payload,
            } => {
                assert_eq!(id, "abc-1");
                assert_eq!(action, "BootNotification");
                assert_eq!(payload["chargePointVendor"], "A");
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn result_roundtrip() {
        let f = Frame::Result {
            id: "42".into(),
            payload: json!({"currentTime":"2026-04-21T00:00:00Z","interval":30,"status":"Accepted"}),
        };
        let wire = f.to_wire();
        assert!(wire.starts_with("[3,"));
        let parsed = Frame::from_wire(&wire).unwrap();
        match parsed {
            Frame::Result { id, payload } => {
                assert_eq!(id, "42");
                assert_eq!(payload["interval"], 30);
            }
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn error_roundtrip_with_details() {
        let wire = r#"[4,"id-1","NotImplemented","action missing",{"foo":1}]"#;
        let parsed = Frame::from_wire(wire).unwrap();
        match parsed {
            Frame::Error {
                id,
                code,
                description,
                details,
            } => {
                assert_eq!(id, "id-1");
                assert_eq!(code, "NotImplemented");
                assert_eq!(description, "action missing");
                assert_eq!(details["foo"], 1);
            }
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn error_without_details_defaults_to_empty_object() {
        let wire = r#"[4,"x","FormationViolation","bad"]"#;
        let parsed = Frame::from_wire(wire).unwrap();
        match parsed {
            Frame::Error { details, .. } => assert!(details.is_object()),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn reject_wrong_arity() {
        assert!(Frame::from_wire(r#"[2,"id","Action"]"#).is_err());
    }

    #[test]
    fn reject_unknown_type() {
        assert!(Frame::from_wire(r#"[9,"id",{}]"#).is_err());
    }

    #[test]
    fn new_call_generates_unique_ids() {
        let (id1, _) = Frame::new_call("Heartbeat", json!({}));
        let (id2, _) = Frame::new_call("Heartbeat", json!({}));
        assert_ne!(id1, id2);
    }
}
