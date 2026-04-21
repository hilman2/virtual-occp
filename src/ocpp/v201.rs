//! OCPP 2.0.1 messages (selected actions for the simulator).
#![allow(non_snake_case, dead_code, clippy::upper_case_acronyms)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- BootNotification ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargingStation {
    pub model: String,
    pub vendorName: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serialNumber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmwareVersion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootNotificationReq {
    pub reason: String, // PowerUp | ApplicationReset | ...
    pub chargingStation: ChargingStation,
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
    pub timestamp: String,
    pub connectorStatus: String, // Available | Occupied | Reserved | Unavailable | Faulted
    pub evseId: i32,
    pub connectorId: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusNotificationResp {}

// ---------- Authorize ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdToken {
    pub idToken: String,
    #[serde(rename = "type")]
    pub kind: String, // Central | eMAID | ISO14443 | ISO15693 | KeyCode | Local | MacAddress | NoAuthorization
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeReq {
    pub idToken: IdToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdTokenInfo {
    pub status: String, // Accepted | Blocked | ConcurrentTx | Expired | Invalid | NoCredit | NotAllowedTypeEVSE | NotAtThisLocation | NotAtThisTime | Unknown
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cacheExpiryDateTime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeResp {
    pub idTokenInfo: IdTokenInfo,
}

// ---------- TransactionEvent ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EVSE {
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connectorId: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInfo {
    pub transactionId: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chargingState: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeSpentCharging: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stoppedReason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remoteStartId: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampledValue {
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unitOfMeasure: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterValue {
    pub timestamp: String,
    pub sampledValue: Vec<SampledValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionEventReq {
    pub eventType: String, // Started | Updated | Ended
    pub timestamp: String,
    pub triggerReason: String, // Authorized | CablePluggedIn | ChargingRateChanged | ...
    pub seqNo: i32,
    pub transactionInfo: TransactionInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idToken: Option<IdToken>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evse: Option<EVSE>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meterValue: Option<Vec<MeterValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TransactionEventResp {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idTokenInfo: Option<IdTokenInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn boot_req_has_nested_charging_station() {
        let req = BootNotificationReq {
            reason: "PowerUp".into(),
            chargingStation: ChargingStation {
                model: "VBox".into(),
                vendorName: "VirtualOCPP".into(),
                serialNumber: None,
                firmwareVersion: Some("0.1".into()),
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["reason"], "PowerUp");
        assert_eq!(v["chargingStation"]["model"], "VBox");
        assert_eq!(v["chargingStation"]["firmwareVersion"], "0.1");
    }

    #[test]
    fn authorize_uses_id_token_with_type() {
        let req = AuthorizeReq {
            idToken: IdToken {
                idToken: "DEADBEEF".into(),
                kind: "ISO14443".into(),
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["idToken"]["idToken"], "DEADBEEF");
        assert_eq!(
            v["idToken"]["type"], "ISO14443",
            "`kind` must serialize as JSON field `type`"
        );
    }

    #[test]
    fn transaction_event_started_serializes_meter_value() {
        let req = TransactionEventReq {
            eventType: "Started".into(),
            timestamp: "2026-04-21T10:00:00Z".into(),
            triggerReason: "Authorized".into(),
            seqNo: 0,
            transactionInfo: TransactionInfo {
                transactionId: "tx-1".into(),
                chargingState: Some("Charging".into()),
                timeSpentCharging: Some(0),
                stoppedReason: None,
                remoteStartId: None,
            },
            idToken: None,
            evse: Some(EVSE {
                id: 1,
                connectorId: Some(1),
            }),
            meterValue: Some(vec![MeterValue {
                timestamp: "2026-04-21T10:00:00Z".into(),
                sampledValue: vec![SampledValue {
                    value: 12345.0,
                    context: Some("Transaction.Begin".into()),
                    measurand: Some("Energy.Active.Import.Register".into()),
                    phase: None,
                    location: None,
                    unitOfMeasure: Some(json!({"unit":"Wh"})),
                }],
            }]),
            offline: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["eventType"], "Started");
        assert_eq!(v["meterValue"][0]["sampledValue"][0]["value"], 12345.0);
        assert_eq!(
            v["meterValue"][0]["sampledValue"][0]["unitOfMeasure"]["unit"],
            "Wh"
        );
    }
}
