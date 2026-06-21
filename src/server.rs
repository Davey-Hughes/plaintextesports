//! Leptos server functions. The bodies read the in-memory cache on the server;
//! on the client the `#[server]` macro replaces them with a network call.

use crate::types::ScheduleView;
use leptos::prelude::*;

/// Homepage schedule: live matches + upcoming days. `game` is "all"/"cs2"/"lol".
#[server(GetSchedule, "/api")]
pub async fn get_schedule(game: String) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::homepage_view(&game))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = game;
        Ok(ScheduleView::default())
    }
}

/// Single-day schedule for `date` ("YYYY-MM-DD"), filtered by `game`.
#[server(GetDay, "/api")]
pub async fn get_day(date: String, game: String) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::day_view(&date, &game))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (date, game);
        Ok(ScheduleView::default())
    }
}
