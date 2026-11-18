// CONSTRAINT: Requires a disposable `LiveKit` development server; see the README.
use livekit_api::services::room::RoomClient;
use station_access::{Credentials, new_room_name};

#[tokio::test]
#[ignore = "requires LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET for a development server"]
async fn recording_announcement_creates_room_and_updates_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let credentials = Credentials::from_environment()?;
    let room = new_room_name();
    let client = RoomClient::with_api_key(
        &credentials.url,
        &std::env::var("LIVEKIT_API_KEY")?,
        &std::env::var("LIVEKIT_API_SECRET")?,
    );
    let result = async {
        for recording in [true, false] {
            credentials.announce_recording(&room, recording).await?;
            let rooms = client.list_rooms(vec![room.clone()]).await?;
            assert_eq!(rooms.len(), 1);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&rooms[0].metadata)?,
                serde_json::json!({"recording":recording})
            );
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    client.delete_room(&room).await?;
    result
}
