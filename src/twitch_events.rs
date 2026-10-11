use serde::Deserialize;
use std::fmt::{Display, Formatter};

const COMMUNITY_POINTS_TOPIC_PREFIX: &str = "community-points-user-v1.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TwitchObservedEvent {
    PointsEarned {
        channel_id: String,
        points_earned: u64,
        balance: u64,
        reason_code: String,
    },
    PointsSpent {
        channel_id: String,
        balance: u64,
    },
    ClaimAvailable {
        channel_id: String,
    },
}

#[derive(Debug)]
pub enum PubSubParseError {
    Json(serde_json::Error),
    MissingField(&'static str),
}

impl Display for PubSubParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid PubSub JSON: {error}"),
            Self::MissingField(field) => write!(formatter, "missing PubSub field: {field}"),
        }
    }
}

impl std::error::Error for PubSubParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::MissingField(_) => None,
        }
    }
}

impl From<serde_json::Error> for PubSubParseError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Deserialize)]
struct PubSubFrame {
    #[serde(rename = "type")]
    message_type: String,
    data: Option<PubSubPayload>,
}

#[derive(Deserialize)]
struct PubSubPayload {
    topic: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct PointsMessage {
    #[serde(rename = "type")]
    message_type: String,
    data: Option<PointsEventData>,
}

#[derive(Deserialize)]
struct PointsEventData {
    point_gain: Option<PointGain>,
    balance: Option<PointBalance>,
    claim: Option<Claim>,
}

#[derive(Deserialize)]
struct PointGain {
    reason_code: Option<String>,
    total_points: Option<u64>,
}

#[derive(Deserialize)]
struct PointBalance {
    channel_id: Option<String>,
    balance: Option<u64>,
}

#[derive(Deserialize)]
struct Claim {
    channel_id: Option<String>,
}

pub fn parse_pubsub_frame(
    frame_json: &str,
) -> Result<Option<TwitchObservedEvent>, PubSubParseError> {
    let frame: PubSubFrame = serde_json::from_str(frame_json)?;
    if frame.message_type != "MESSAGE" {
        return Ok(None);
    }

    let payload = frame.data.ok_or(PubSubParseError::MissingField("data"))?;
    let topic = payload
        .topic
        .ok_or(PubSubParseError::MissingField("data.topic"))?;
    if !topic.starts_with(COMMUNITY_POINTS_TOPIC_PREFIX) {
        return Ok(None);
    }

    let message_json = payload
        .message
        .ok_or(PubSubParseError::MissingField("data.message"))?;
    let message: PointsMessage = serde_json::from_str(&message_json)?;
    let observed_event =
        match message.message_type.as_str() {
            "points-earned" => {
                let data = message
                    .data
                    .ok_or(PubSubParseError::MissingField("message.data"))?;
                let point_gain = data
                    .point_gain
                    .ok_or(PubSubParseError::MissingField("message.data.point_gain"))?;
                let balance = data
                    .balance
                    .ok_or(PubSubParseError::MissingField("message.data.balance"))?;
                TwitchObservedEvent::PointsEarned {
                    channel_id: balance.channel_id.ok_or(PubSubParseError::MissingField(
                        "message.data.balance.channel_id",
                    ))?,
                    balance: balance.balance.ok_or(PubSubParseError::MissingField(
                        "message.data.balance.balance",
                    ))?,
                    points_earned: point_gain.total_points.ok_or(
                        PubSubParseError::MissingField("message.data.point_gain.total_points"),
                    )?,
                    reason_code: point_gain
                        .reason_code
                        .ok_or(PubSubParseError::MissingField(
                            "message.data.point_gain.reason_code",
                        ))?,
                }
            }
            "points-spent" => {
                let data = message
                    .data
                    .ok_or(PubSubParseError::MissingField("message.data"))?;
                let balance = data
                    .balance
                    .ok_or(PubSubParseError::MissingField("message.data.balance"))?;
                TwitchObservedEvent::PointsSpent {
                    channel_id: balance.channel_id.ok_or(PubSubParseError::MissingField(
                        "message.data.balance.channel_id",
                    ))?,
                    balance: balance.balance.ok_or(PubSubParseError::MissingField(
                        "message.data.balance.balance",
                    ))?,
                }
            }
            "claim-available" => {
                let data = message
                    .data
                    .ok_or(PubSubParseError::MissingField("message.data"))?;
                let claim = data
                    .claim
                    .ok_or(PubSubParseError::MissingField("message.data.claim"))?;
                TwitchObservedEvent::ClaimAvailable {
                    channel_id: claim.channel_id.ok_or(PubSubParseError::MissingField(
                        "message.data.claim.channel_id",
                    ))?,
                }
            }
            _ => return Ok(None),
        };

    Ok(Some(observed_event))
}

#[cfg(test)]
mod tests {
    use super::{parse_pubsub_frame, PubSubParseError, TwitchObservedEvent};

    fn frame(topic: &str, message: &str) -> String {
        serde_json::json!({
            "type": "MESSAGE",
            "data": {
                "topic": topic,
                "message": message,
            }
        })
        .to_string()
    }

    #[test]
    fn parses_points_earned_balance_and_reason() {
        let message = serde_json::json!({
            "type": "points-earned",
            "data": {
                "point_gain": {"reason_code": "WATCH", "total_points": 12},
                "balance": {"channel_id": "98765", "balance": 112},
            },
        })
        .to_string();

        assert_eq!(
            parse_pubsub_frame(&frame("community-points-user-v1.123", &message)).unwrap(),
            Some(TwitchObservedEvent::PointsEarned {
                channel_id: "98765".to_owned(),
                points_earned: 12,
                balance: 112,
                reason_code: "WATCH".to_owned(),
            })
        );
    }

    #[test]
    fn parses_watch_streak_as_a_points_earned_event() {
        let message = serde_json::json!({
            "type": "points-earned",
            "data": {
                "point_gain": {"reason_code": "WATCH_STREAK", "total_points": 400},
                "balance": {"channel_id": "98765", "balance": 1234},
            },
        })
        .to_string();

        assert_eq!(
            parse_pubsub_frame(&frame("community-points-user-v1.123", &message)).unwrap(),
            Some(TwitchObservedEvent::PointsEarned {
                channel_id: "98765".to_owned(),
                points_earned: 400,
                balance: 1234,
                reason_code: "WATCH_STREAK".to_owned(),
            })
        );
    }

    #[test]
    fn parses_points_spent_balance() {
        let message = serde_json::json!({
            "type": "points-spent",
            "data": {"balance": {"channel_id": "98765", "balance": 80}},
        })
        .to_string();

        assert_eq!(
            parse_pubsub_frame(&frame("community-points-user-v1.123", &message)).unwrap(),
            Some(TwitchObservedEvent::PointsSpent {
                channel_id: "98765".to_owned(),
                balance: 80,
            })
        );
    }

    #[test]
    fn parses_bonus_claim_availability_without_claiming_it() {
        let message = serde_json::json!({
            "type": "claim-available",
            "data": {"claim": {"channel_id": "98765", "id": "claim-1"}},
        })
        .to_string();

        assert_eq!(
            parse_pubsub_frame(&frame("community-points-user-v1.123", &message)).unwrap(),
            Some(TwitchObservedEvent::ClaimAvailable {
                channel_id: "98765".to_owned(),
            })
        );
    }

    #[test]
    fn ignores_unrelated_topics_and_event_types() {
        assert_eq!(
            parse_pubsub_frame(&frame("video-playback-by-id.98765", "{}")).unwrap(),
            None
        );
        let message = serde_json::json!({"type": "prediction-event", "data": {}}).to_string();
        assert_eq!(
            parse_pubsub_frame(&frame("community-points-user-v1.123", &message)).unwrap(),
            None
        );
        let unknown_without_data = serde_json::json!({"type": "prediction-event"}).to_string();
        assert_eq!(
            parse_pubsub_frame(&frame(
                "community-points-user-v1.123",
                &unknown_without_data
            ))
            .unwrap(),
            None
        );
        assert_eq!(parse_pubsub_frame(r#"{"type":"PONG"}"#).unwrap(), None);
    }

    #[test]
    fn rejects_points_events_missing_required_fields() {
        let earned = serde_json::json!({
            "type": "points-earned",
            "data": {
                "point_gain": {"reason_code": "WATCH", "total_points": 12},
                "balance": {"balance": 100},
            },
        })
        .to_string();
        assert!(matches!(
            parse_pubsub_frame(&frame("community-points-user-v1.123", &earned)),
            Err(PubSubParseError::MissingField(
                "message.data.balance.channel_id"
            ))
        ));

        let spent = serde_json::json!({"type": "points-spent", "data": {}}).to_string();
        assert!(matches!(
            parse_pubsub_frame(&frame("community-points-user-v1.123", &spent)),
            Err(PubSubParseError::MissingField("message.data.balance"))
        ));
    }
}
