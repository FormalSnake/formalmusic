//! `account/accounts_list` (the account switcher) and `account/account_menu`
//! (the avatar menu). Written from the known shape of these responses; there
//! is no recorded fixture since they need a session.

use super::{missing, text, thumbnails};
use crate::Result;
use formalmusic_api::{Account, SessionInfo};
use serde_json::Value;

/// Every `accountItem` under the switcher menu, wherever YouTube nests it.
pub fn parse_accounts(json: &Value) -> Result<Vec<Account>> {
    let mut found = Vec::new();
    collect_account_items(&json["actions"], &mut found);
    if found.is_empty() {
        return Err(missing("actions[0].getMultiPageMenuAction...accountItem"));
    }
    Ok(found.into_iter().filter_map(account_item).collect())
}

fn collect_account_items<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "accountItem" {
                    out.push(child);
                } else {
                    collect_account_items(child, out);
                }
            }
        }
        Value::Array(list) => list
            .iter()
            .for_each(|child| collect_account_items(child, out)),
        _ => {}
    }
}

fn account_item(item: &Value) -> Option<Account> {
    let tokens =
        item["serviceEndpoint"]["selectActiveIdentityEndpoint"]["supportedTokens"].as_array();
    let page_id = tokens
        .into_iter()
        .flatten()
        .find_map(|t| t["pageIdToken"]["pageId"].as_str())
        .map(str::to_owned);
    Some(Account {
        name: text(&item["accountName"])?,
        handle: text(&item["channelHandle"])
            .or_else(|| text(&item["accountByline"]).filter(|b| b.starts_with('@'))),
        thumbnails: thumbnails(&item["accountPhoto"]),
        page_id,
        selected: item["isSelected"].as_bool().unwrap_or(false),
    })
}

/// The signed-in account from the avatar menu. Premium is not in this
/// response; [`crate::parse::player::has_premium_audio`] tells.
///
/// Cookies YouTube no longer accepts, such as ones exported from a browser
/// that has since rotated them, get the signed-out menu: no account header,
/// and `logged_in` 0 in the tracking params.
pub fn parse_session(json: &Value) -> Result<SessionInfo> {
    let header = &json["actions"][0]["openPopupAction"]["popup"]["multiPageMenuRenderer"]["header"]
        ["activeAccountHeaderRenderer"];
    if !header.is_object() && logged_in(json) == Some(false) {
        return Ok(SessionInfo::default());
    }
    if !header.is_object() {
        return Err(missing(
            "actions[0].openPopupAction.popup.multiPageMenuRenderer.header.activeAccountHeaderRenderer",
        ));
    }
    let account = Account {
        name: text(&header["accountName"])
            .ok_or_else(|| missing("activeAccountHeaderRenderer.accountName"))?,
        handle: text(&header["channelHandle"]),
        thumbnails: thumbnails(&header["accountPhoto"]),
        page_id: None,
        selected: true,
    };
    Ok(SessionInfo {
        signed_in: true,
        account: Some(account),
        premium: false,
    })
}

/// The `logged_in` flag YouTube reports in `responseContext`.
fn logged_in(json: &Value) -> Option<bool> {
    json["responseContext"]["serviceTrackingParams"]
        .as_array()?
        .iter()
        .filter_map(|service| service["params"].as_array())
        .flatten()
        .find(|param| param["key"] == "logged_in")
        .and_then(|param| param["value"].as_str())
        .map(|value| value == "1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accounts_from_known_shape() {
        let json = json!({"actions": [{"getMultiPageMenuAction": {"menu": {"multiPageMenuRenderer": {"sections": [
            {"accountSectionListRenderer": {"contents": [{"accountItemSectionRenderer": {"contents": [
                {"accountItem": {"accountName": {"simpleText": "Kyan"}, "isSelected": true,
                    "accountPhoto": {"thumbnails": [{"url": "https://x/a.jpg", "width": 88, "height": 88}]},
                    "channelHandle": {"simpleText": "@kyan"},
                    "serviceEndpoint": {"selectActiveIdentityEndpoint": {"supportedTokens": [{"accountStateToken": {}}]}}}},
                {"accountItem": {"accountName": {"simpleText": "Brand"}, "isSelected": false,
                    "serviceEndpoint": {"selectActiveIdentityEndpoint": {"supportedTokens": [{"pageIdToken": {"pageId": "1234"}}]}}}},
                {"compactLinkRenderer": {}}
            ]}}]}}
        ]}}}}]});
        let accounts = parse_accounts(&json).unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(accounts[0].selected);
        assert_eq!(accounts[0].handle.as_deref(), Some("@kyan"));
        assert_eq!(accounts[1].page_id.as_deref(), Some("1234"));
    }

    #[test]
    fn session_from_avatar_menu() {
        let json = json!({
            "actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {"header": {"activeAccountHeaderRenderer": {
                "accountName": {"runs": [{"text": "Kyan"}]}, "channelHandle": {"runs": [{"text": "@kyan"}]},
                "accountPhoto": {"thumbnails": [{"url": "https://x/a.jpg", "width": 88, "height": 88}]}
            }}}}}}]
        });
        let session = parse_session(&json).unwrap();
        assert!(session.signed_in);
        assert_eq!(session.account.unwrap().handle.as_deref(), Some("@kyan"));
    }

    #[test]
    fn signed_out_menu_is_not_an_account() {
        let json = json!({
            "responseContext": {"serviceTrackingParams": [
                {"service": "GFEEDBACK", "params": [{"key": "logged_in", "value": "0"}]}
            ]},
            "actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {"sections": []}}}}]
        });
        assert_eq!(parse_session(&json).unwrap(), SessionInfo::default());
        let unknown =
            json!({"actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {}}}}]});
        assert!(parse_session(&unknown).is_err());
    }
}
