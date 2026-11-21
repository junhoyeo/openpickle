//! Terminal entry point until the GPUI login screen can receive OAuth callbacks.
use openpickle_api::{ClientOptions, PickleClient, kakao::KakaoConfig};
use openpickle_desktop::store::{Session, Store};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open()?;
    let _claim = store
        .claim()?
        .ok_or("Quit OpenPickle before signing in from the terminal")?;
    let mut config = KakaoConfig::default();
    if let Ok(value) = std::env::var("PICKLE_KAKAO_CLIENT_ID") {
        config.redirect_uri = format!("kakao{value}://oauth");
        config.client_id = value;
    }
    if let Ok(value) = std::env::var("PICKLE_KAKAO_REDIRECT_URI") {
        config.redirect_uri = value;
    }
    if let Ok(value) = std::env::var("PICKLE_KAKAO_IOS_BUNDLE_ID") {
        config.ios_bundle_id = value;
    }
    if let Ok(value) = std::env::var("PICKLE_KAKAO_KA") {
        config.ka = value;
    }
    let device_id = uuid::Uuid::new_v4().to_string();
    let client = PickleClient::new(ClientOptions {
        device_id: Some(device_id.clone()),
        ..Default::default()
    })?;
    let pending = client.request_kakao_authorize_url(&config)?;
    println!("{}", pending.url);
    eprintln!(
        "Open this URL in a browser. Capture {} and paste the redirect URL (or code), then press Enter:",
        config.redirect_uri
    );
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let code = config.parse_code(&input)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let issued = runtime.block_on(async {
        let id_token = client
            .exchange_kakao_code(&config, &code, &pending.verifier)
            .await?;
        // Pickle errors can include server-controlled detail; keep terminal errors credential-free.
        client
            .login_with_kakao_id_token(&id_token)
            .await
            .map_err(|_| -> Box<dyn std::error::Error> { "Pickle Kakao login failed".into() })
    })?;
    store.start_session(&Session {
        login_session_token: issued.login_session_token,
        device_id,
    })?;
    println!(
        "{}",
        serde_json::json!({ "user_id": issued.user_id, "expires_at": issued.expires_at })
    );
    Ok(())
}
