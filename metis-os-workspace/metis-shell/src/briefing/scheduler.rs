use std::time::Duration;

use crate::briefing::item::{BriefingItem, load_briefing_config};
use crate::briefing::{fetch_headlines, fetch_summary};
use crate::state::EventPublisher;

pub struct BriefingScheduler;

impl BriefingScheduler {
    pub fn spawn(events: EventPublisher) {
        std::thread::Builder::new()
            .name("metis-briefing".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("briefing runtime");
                runtime.block_on(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if let Ok(items) = run_connectors().await {
                        events.publish(crate::state::SystemEvent::BriefingReady(items));
                    }
                });
            })
            .expect("briefing thread");
    }
}

pub async fn run_connectors() -> Result<Vec<BriefingItem>, String> {
    let cfg = load_briefing_config();
    let mut items = Vec::new();

    match fetch_summary(cfg.weather.latitude, cfg.weather.longitude).await {
        Ok(summary) => items.push(BriefingItem {
            id: "weather".into(),
            title: "Weather".into(),
            body: summary,
            source: "open-meteo".into(),
        }),
        Err(err) => items.push(BriefingItem {
            id: "weather".into(),
            title: "Weather".into(),
            body: format!("Unavailable: {err}"),
            source: "open-meteo".into(),
        }),
    }

    match fetch_headlines(&cfg.rss.feed_url).await {
        Ok(headlines) => {
            for (idx, headline) in headlines.into_iter().take(5).enumerate() {
                items.push(BriefingItem {
                    id: format!("rss-{idx}"),
                    title: headline.clone(),
                    body: headline,
                    source: "rss".into(),
                });
            }
        }
        Err(err) => items.push(BriefingItem {
            id: "rss".into(),
            title: "Headlines".into(),
            body: format!("Feed unavailable: {err}"),
            source: "rss".into(),
        }),
    }

    Ok(items)
}
