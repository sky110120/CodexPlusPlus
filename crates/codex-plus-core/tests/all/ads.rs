use codex_plus_core::ads::{
    DEFAULT_AD_LIST_URLS, cache_busted_ad_url, fetch_ad_list, fetch_ad_list_from_urls, normalize_ad_payload,
};
use serde_json::json;

#[test]
fn ad_sources_are_disabled() {
    assert!(DEFAULT_AD_LIST_URLS.is_empty());
}

#[test]
fn cache_busted_ad_url_appends_version_query_to_plain_url() {
    assert_eq!(
        cache_busted_ad_url("https://example.test/ads.json", 1779035222758),
        "https://example.test/ads.json?v=1779035222758"
    );
}

#[test]
fn cache_busted_ad_url_preserves_existing_query() {
    assert_eq!(
        cache_busted_ad_url("https://example.test/ads.json?source=cdn", 1779035222758),
        "https://example.test/ads.json?source=cdn&v=1779035222758"
    );
}

#[test]
fn normalizes_remote_ads_for_plugin_and_manager_rendering() {
    let payload = normalize_ad_payload(json!({
        "version": 1,
        "ads": [
            {
                "id": "sponsor",
                "type": "sponsor",
                "title": "赞助商",
                "description": "推荐内容",
                "url": "https://example.test",
                "highlights": ["稳定"]
            },
            {
                "id": "normal",
                "type": "normal",
                "title": "普通推荐",
                "description": "推荐内容",
                "url": "https://example.org"
            },
            {
                "id": "broken",
                "type": "normal",
                "title": "",
                "description": "missing title",
                "url": "https://example.invalid"
            }
        ]
    }));

    assert_eq!(payload["version"], json!(1));
    assert_eq!(payload["ads"].as_array().unwrap().len(), 2);
    assert_eq!(payload["ads"][0]["type"], json!("sponsor"));
    assert_eq!(payload["ads"][1]["type"], json!("normal"));
}

#[test]
fn drops_ads_whose_destination_is_not_a_web_url() {
    let payload = normalize_ad_payload(json!({
        "version": 1,
        "ads": [
            {
                "id": "ok",
                "type": "normal",
                "title": "正常",
                "description": "普通推荐",
                "url": "https://example.test/partner"
            },
            {
                "id": "javascript",
                "type": "normal",
                "title": "脚本",
                "description": "普通推荐",
                "url": "javascript:alert(document.domain)"
            },
            {
                "id": "data",
                "type": "normal",
                "title": "数据",
                "description": "普通推荐",
                "url": "data:text/html,<script>alert(1)</script>"
            },
            {
                "id": "file",
                "type": "normal",
                "title": "本地文件",
                "description": "普通推荐",
                "url": "file:///etc/passwd"
            },
            {
                "id": "scheme-relative",
                "type": "normal",
                "title": "协议相对",
                "description": "普通推荐",
                "url": "//example.test/partner"
            },
            {
                "id": "uppercase",
                "type": "normal",
                "title": "大写 scheme",
                "description": "普通推荐",
                "url": "HTTPS://example.test/partner"
            }
        ],
        "top_ad": {
            "id": "top-javascript",
            "type": "sponsor",
            "title": "置顶",
            "description": "置顶推荐",
            "url": "javascript:alert(1)"
        }
    }));

    let ids = payload["ads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|ad| ad["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["ok", "uppercase"]);
    assert!(payload.get("topAd").is_none(), "非 http(s) 的置顶位必须丢弃");
}

#[test]
fn builtin_sponsors_are_not_appended_to_remote_ads() {
    let payload = normalize_ad_payload(json!({
        "version": 1,
        "ads": [
            {
                "id": "remote-sponsor",
                "type": "sponsor",
                "title": "远端赞助商",
                "description": "远端推荐内容",
                "url": "https://example.test"
            },
            {
                "id": "remote-normal",
                "type": "normal",
                "title": "普通推荐",
                "description": "普通推荐内容",
                "url": "https://example.org"
            }
        ]
    }));
    let ads = payload["ads"].as_array().unwrap();

    assert_eq!(ads.len(), 2);
    assert_eq!(ads[0]["id"], json!("remote-sponsor"));
    assert_eq!(ads[1]["id"], json!("remote-normal"));
}

#[test]
fn ad_images_come_only_from_the_remote_list() {
    let payload = normalize_ad_payload(json!({
        "version": 1,
        "ads": [
            {
                "id": "apikey-fun-ai-relay",
                "type": "sponsor",
                "title": "APIKEY.FUN",
                "description": "远端推荐内容",
                "url": "https://example.test/apikey"
            },
            {
                "id": "jojocode-codex-relay",
                "type": "sponsor",
                "title": "JOJO Code",
                "description": "远端推荐内容",
                "url": "https://example.test/jojocode",
                "image": "https://example.test/logo.png"
            }
        ]
    }));
    let ads = payload["ads"].as_array().unwrap();

    assert!(ads[0].get("image").is_none());
    assert_eq!(ads[1]["image"], json!("https://example.test/logo.png"));
}

#[tokio::test]
async fn old_ad_clients_get_an_empty_list_without_reading_any_source() {
    struct UnreadableUrl;
    impl AsRef<str> for UnreadableUrl {
        fn as_ref(&self) -> &str {
            panic!("disabled ad transport must not inspect or contact any source")
        }
    }
    let empty = json!({"version": 1, "ads": []});
    assert_eq!(fetch_ad_list().await.unwrap(), empty);
    assert_eq!(fetch_ad_list_from_urls(&[UnreadableUrl]).await.unwrap(), empty);
}
