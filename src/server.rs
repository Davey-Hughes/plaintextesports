//! Leptos server functions. The bodies read the in-memory cache on the server;
//! on the client the `#[server]` macro replaces them with a network call.

use crate::types::ScheduleView;
use leptos::prelude::*;

/// Homepage schedule. `game` is "all"/"cs2"/"lol"; `tz` is an IANA name (empty
/// = server default); `hour24` selects 24h vs 12h time formatting.
#[server(GetSchedule, "/api")]
pub async fn get_schedule(
    game: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::homepage_view(&game, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (game, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// Single-day schedule for `date` ("YYYY-MM-DD").
#[server(GetDay, "/api")]
pub async fn get_day(
    date: String,
    game: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::day_view(&date, &game, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (date, game, tz, hour24);
        Ok(ScheduleView::default())
    }
}
