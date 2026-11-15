use serde_json::json;

use super::*;

#[test]
fn asks_one_score_and_one_choice_about_the_trimmed_request() {
    let long = "x".repeat(10_000);
    let b = body(&format!("  {long}  "));
    assert_eq!(b["model"], MODEL);
    assert_eq!(b["state"].as_str().unwrap().chars().count(), 6000);
    assert_eq!(b["questions"]["difficulty"]["type"], "score");
    assert_eq!(
        b["questions"]["difficulty"]["criteria"]
            .as_array()
            .unwrap()
            .len(),
        LEVELS.len()
    );
    assert_eq!(b["questions"]["kind"]["type"], "choice");
    assert_eq!(
        b["questions"]["kind"]["criteria"]
            .as_object()
            .unwrap()
            .len(),
        KINDS.len()
    );
}

#[test]
fn reads_the_answers_and_estimates_the_cost() {
    let v = json!({
        "model": "jev-1.14.0",
        "answers": {
            "difficulty": {
                "type": "score",
                "score": 2.7,
                "confidence": 0.82,
                "probabilities": [0.0, 0.05, 0.25, 0.6, 0.1]
            },
            "kind": {
                "type": "choice",
                "choice": "refactor",
                "confidence": 0.91,
                "probabilities": { "refactor": 0.91, "feature": 0.09 }
            }
        },
        "usage": { "input_tokens": 1000, "output_tokens": 0 }
    });
    let got = parse(&v).unwrap();
    assert!((got.difficulty - 2.7).abs() < 1e-9);
    assert!((got.difficulty_confidence - 0.82).abs() < 1e-9);
    assert_eq!(got.kind, "refactor");
    assert_eq!(got.model.as_deref(), Some("jev-1.14.0"));
    assert!((got.cost_usd.unwrap() - 0.000_042).abs() < 1e-12);
}

#[test]
fn prefers_the_reported_cost_and_refuses_unknown_answers() {
    let mut v = json!({
        "answers": {
            "difficulty": { "score": "9" },
            "kind": { "choice": "fix" }
        },
        "meta": { "usd_spent": 0.0003 }
    });
    let got = parse(&v).unwrap();
    assert!((got.difficulty - 4.0).abs() < 1e-9);
    assert!((got.difficulty_confidence - 1.0).abs() < 1e-9);
    assert!((got.cost_usd.unwrap() - 0.0003).abs() < 1e-12);
    v["answers"]["kind"]["choice"] = json!("poetry");
    assert!(parse(&v).is_none());
    assert!(parse(&json!({})).is_none());
}

#[test]
fn maps_errors_to_what_the_user_can_do() {
    let missing = json!({
        "detail": {
            "error_type": "authentication_error",
            "message": "Must supply an API key! Check your request and try again."
        }
    });
    assert_eq!(failure(403, &missing), Failure::Rejected);
    assert_eq!(failure(401, &json!({})), Failure::Rejected);
    assert_eq!(failure(402, &json!({})), Failure::Credits);
    assert_eq!(
        failure(
            400,
            &json!({ "detail": { "error_type": "insufficient_credits" } })
        ),
        Failure::Credits
    );
    assert_eq!(failure(429, &json!({})), Failure::Limited);
    assert_eq!(
        failure(500, &json!({ "detail": "boom" })),
        Failure::Server(500, Some("boom".into()))
    );
    assert!(Failure::Credits.zh().contains("充值"));
    assert_eq!(Failure::Timeout.en(), "timed out");
}
