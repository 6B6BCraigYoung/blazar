use super::*;

fn options() -> Value {
    json!([
        { "id": "mode", "category": "mode", "type": "select", "currentValue": "accept-edits",
          "options": [{ "value": "accept-edits", "name": "Code" }] },
        { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "swe-2-flash-medium",
          "options": [
              { "value": "swe-2-flash-medium", "name": "SWE-2 Flash Medium", "_meta": { "cognition.ai/supportsImages": false } },
              { "value": "swe-1-6-slow", "name": "SWE-1.6 Slow", "_meta": { "cognition.ai/supportsImages": true } }
          ] }
    ])
}

#[test]
fn the_menu_lists_only_the_models_this_account_can_use() {
    let list = parse_options(&options()).unwrap();
    assert_eq!(list.current.as_deref(), Some("swe-2-flash-medium"));
    let menu = rows(&list);
    let ids: Vec<&str> = menu.iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(ids, ["", "swe-2-flash-medium", "swe-1-6-slow"]);
    assert_eq!(menu[0]["desc"], "Uses SWE-2 Flash Medium");
    assert_eq!(menu[0]["images"], false, "默认型号看不了图");
    assert_eq!(menu[2]["images"], true);
    assert_eq!(menu[1]["efforts"], json!([]), "档位已经写在型号名里");
    assert!(parse_options(&json!([{ "id": "mode", "options": [] }])).is_none());
    assert_eq!(
        rows(&Listing::default())[0]["desc"],
        "Uses Devin's default model"
    );
}

#[test]
fn runs_keep_the_menu_fresh_without_moving_the_default() {
    let new = json!({ "jsonrpc": "2.0", "id": "blazar-new",
                      "result": { "sessionId": "maddening-mallet", "configOptions": options() } });
    remember(&new.to_string());
    let switched = json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "maddening-mallet",
        "update": { "sessionUpdate": "config_option_update", "configOptions": [
            { "id": "model", "currentValue": "swe-1-6-slow", "options": [
                { "value": "swe-2-flash-medium", "name": "SWE-2 Flash Medium" },
                { "value": "swe-1-6-slow", "name": "SWE-1.6 Slow" },
                { "value": "swe-2-max", "name": "SWE-2 Max" }
            ] }
        ] } } });
    remember(&switched.to_string());
    let (_, list) = CACHE.lock().unwrap().clone().unwrap();
    assert_eq!(list.models.len(), 3, "账号能用的型号变了就跟着变");
    assert_eq!(
        list.current.as_deref(),
        Some("swe-2-flash-medium"),
        "某个会话换了型号不等于默认型号变了"
    );
    remember("{\"configOptions\": broken");
    remember("{}");
}
