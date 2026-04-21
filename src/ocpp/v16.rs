//! OCPP 1.6-J messages (selected actions for the simulator).
#![allow(non_snake_case, dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- BootNotification ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootNotificationReq {
    pub chargePointVendor: String,
    pub chargePointModel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chargePointSerialNumber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmwareVersion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iccid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imsi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meterType: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meterSerialNumber: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootNotificationResp {
    pub currentTime: String,
    pub interval: i32,
    pub status: String, // Accepted | Pending | Rejected
}

// ---------- Heartbeat ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeartbeatReq {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResp {
    pub currentTime: String,
}

// ---------- StatusNotification ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusNotificationReq {
    pub connectorId: i32,
    pub errorCode: String, // z.B. "NoError"
    pub status: String,    // Available | Preparing | Charging | ... | Unavailable | Faulted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusNotificationResp {}

// ---------- Authorize ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeReq {
    pub idTag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdTagInfo {
    pub status: String, // Accepted | Blocked | Expired | Invalid | ConcurrentTx
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiryDate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parentIdTag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeResp {
    pub idTagInfo: IdTagInfo,
}

// ---------- StartTransaction ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartTransactionReq {
    pub connectorId: i32,
    pub idTag: String,
    pub meterStart: i32,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reservationId: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartTransactionResp {
    pub transactionId: i32,
    pub idTagInfo: IdTagInfo,
}

// ---------- StopTransaction ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopTransactionReq {
    pub transactionId: i32,
    pub idTag: Option<String>,
    pub meterStop: i32,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transactionData: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopTransactionResp {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idTagInfo: Option<IdTagInfo>,
}

// ---------- MeterValues ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampledValue {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterValueEntry {
    pub timestamp: String,
    pub sampledValue: Vec<SampledValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterValuesReq {
    pub connectorId: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transactionId: Option<i32>,
    pub meterValue: Vec<MeterValueEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MeterValuesResp {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn boot_req_serializes_camel_case_fields() {
        let req = BootNotificationReq {
            chargePointVendor: "VirtualOCPP".into(),
            chargePointModel: "VBox-2".into(),
            chargePointSerialNumber: Some("SN-1".into()),
            firmwareVersion: Some("0.1.0".into()),
            iccid: None,
            imsi: None,
            meterType: None,
            meterSerialNumber: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["chargePointVendor"], "VirtualOCPP");
        assert_eq!(v["chargePointModel"], "VBox-2");
        assert_eq!(v["chargePointSerialNumber"], "SN-1");
        assert!(v.get("iccid").is_none(), "None fields must be skipped");
    }

    #[test]
    fn boot_resp_deserializes_csms_format() {
        let raw = json!({"currentTime":"2026-04-21T10:00:00Z","interval":60,"status":"Accepted"});
        let r: BootNotificationResp = serde_json::from_value(raw).unwrap();
        assert_eq!(r.status, "Accepted");
        assert_eq!(r.interval, 60);
    }

    #[test]
    fn authorize_resp_with_id_tag_info() {
        let raw = json!({"idTagInfo":{"status":"Accepted","expiryDate":"2099-01-01T00:00:00Z"}});
        let r: AuthorizeResp = serde_json::from_value(raw).unwrap();
        assert_eq!(r.idTagInfo.status, "Accepted");
        assert!(r.idTagInfo.expiryDate.is_some());
    }

    #[test]
    fn status_notification_minimal() {
        let req = StatusNotificationReq {
            connectorId: 1,
            errorCode: "NoError".into(),
            status: "Available".into(),
            timestamp: None,
            info: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["connectorId"], 1);
        assert_eq!(v["status"], "Available");
        assert!(v.get("timestamp").is_none());
    }
}
