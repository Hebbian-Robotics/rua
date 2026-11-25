use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

fn credentials() -> Result<Credentials, AccessError> {
    Credentials::from_values([
        Some("wss://camera.example".into()),
        Some("key".into()),
        Some("secret".into()),
    ])
}

fn claims(token: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let payload = token.split('.').nth(1).ok_or("JWT payload missing")?;
    Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)?)
}

#[test]
fn tokens_enforce_roles_room_identity_and_lifetime() -> Result<(), Box<dyn std::error::Error>> {
    let credentials = credentials()?;
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    for (lifetime, seconds) in [
        (CameraLifetime::PhoneInvite, 3600),
        (CameraLifetime::Station, 43_200),
    ] {
        let claims = claims(
            &credentials
                .camera_access("station-test", "phone_front", lifetime)?
                .token,
        )?;
        assert_eq!(claims["sub"], "phone_front");
        assert_eq!(claims["iss"], "key");
        assert_eq!(
            claims["video"],
            json!({"roomJoin":true,"room":"station-test","canPublish":true,"canPublishSources":["camera"],"canSubscribe":false,"canPublishData":false})
        );
        assert_eq!(
            claims["exp"].as_u64().ok_or("expiration missing")?
                - claims["nbf"].as_u64().ok_or("start missing")?,
            seconds
        );
        assert!(claims["nbf"].as_u64().ok_or("start missing")? >= before);
    }
    for (role, prefix, seconds) in [
        (WatcherRole::Viewer, "viewer-", 3600),
        (WatcherRole::Model, "model-", 3600),
        (WatcherRole::Recorder, "recorder-", 43_200),
    ] {
        let claims = claims(&credentials.watcher_access("station-test", role)?.token)?;
        let identity = claims["sub"].as_str().ok_or("identity missing")?;
        assert!(
            identity.starts_with(prefix),
            "{identity} should carry role {role:?}"
        );
        assert_eq!(
            claims["video"],
            json!({"roomJoin":true,"room":"station-test","canPublish":false,"canSubscribe":true,"canPublishData":false,"hidden":true})
        );
        assert_eq!(
            claims["exp"].as_u64().ok_or("expiration missing")?
                - claims["nbf"].as_u64().ok_or("start missing")?,
            seconds
        );
    }
    assert!(
        credentials
            .camera_access("room", "../bad", CameraLifetime::Station)
            .is_err()
    );
    Ok(())
}

#[test]
fn independent_viewers_and_stations_have_distinct_identities()
-> Result<(), Box<dyn std::error::Error>> {
    let credentials = credentials()?;
    let room = new_room_name();
    let mut identities = std::collections::BTreeSet::new();
    for _ in 0..3 {
        let token = credentials
            .watcher_access(&room, WatcherRole::Viewer)?
            .token;
        identities.insert(
            claims(&token)?["sub"]
                .as_str()
                .ok_or("identity missing")?
                .to_owned(),
        );
    }
    assert!(
        identities.len() > 1,
        "overlapping viewer sessions need distinct identities"
    );
    assert!(
        [new_room_name(), new_room_name()]
            .iter()
            .any(|other| other != &room),
        "different stations need independent room names"
    );
    Ok(())
}

#[test]
fn credentials_report_all_missing_values_and_distinguish_cameras_off() {
    let Err(error) = Credentials::from_values([None, Some(String::new()), None]) else {
        panic!("missing credentials accepted");
    };
    assert_eq!(
        error.to_string(),
        "Set LiveKit credentials: LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET"
    );
    assert!(matches!(error, AccessError::MissingCredentials(missing) if missing == VARIABLES));
    assert!(matches!(
        CameraConfiguration::from_values([None, None, None]),
        Ok(CameraConfiguration::Off { .. })
    ));
    assert!(
        matches!(CameraConfiguration::from_values([Some("url".into()),None,None]), Err(AccessError::MissingCredentials(missing)) if missing == ["LIVEKIT_API_KEY", "LIVEKIT_API_SECRET"])
    );
}

#[test]
fn invites_are_refused_for_servers_a_phone_cannot_join() -> Result<(), Box<dyn std::error::Error>> {
    for server_url in [
        "ws://127.0.0.1:7880",
        "wss://localhost:7880",
        "ws://[::1]:7880",
        "ws://192.168.1.20:7880",
    ] {
        let credentials = Credentials::from_values([
            Some(server_url.into()),
            Some("key".into()),
            Some("secret".into()),
        ])?;
        let refusal = credentials.phone_invite("station-test", "front", "https://camera.example/");
        assert!(
            matches!(refusal, Err(AccessError::PhoneCannotReachServer(_))),
            "{server_url} should be refused"
        );
    }
    Ok(())
}

#[test]
fn invite_fragment_carries_the_server_url_and_token() -> Result<(), Box<dyn std::error::Error>> {
    let invite = credentials()?.phone_invite(
        "station-test",
        "front",
        "https://station.example/phone.html?theme=dark",
    )?;
    assert_eq!(invite.camera_name, "phone_front");
    let url = url::Url::parse(&invite.url)?;
    assert_eq!(url.query(), Some("theme=dark"));
    let fragment: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(url.fragment().ok_or("fragment missing")?.as_bytes())
            .into_owned()
            .collect();
    assert_eq!(
        fragment.get("url").ok_or("server URL missing")?,
        "wss://camera.example"
    );
    let token = fragment.get("token").ok_or("token missing")?;
    assert!(
        !url.as_str()
            .split('#')
            .next()
            .ok_or("URL missing")?
            .contains(token)
    );
    assert_eq!(claims(token)?["sub"], "phone_front");
    Ok(())
}
