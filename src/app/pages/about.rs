//! The about page.
use crate::app::*;

#[component]
pub(crate) fn AboutPage() -> impl IntoView {
    view! {
        <article class="about">
            <h1>"about"</h1>
            <p>
                "A fast schedule for "
                <strong>"tier-1 Counter-Strike 2 and League of Legends"</strong>
                " — just the top events. Times are in your timezone; your theme and "
                "12h/24h choices are remembered."
            </p>

            <h2>"Results are hidden by default"</h2>
            <p>"So you can browse finished matches without spoilers. To reveal:"</p>
            <ul>
                <li>
                    <span class="kbd">"show scores"</span>
                    " at the top reveals every score, standing, and bracket at once."
                </li>
                <li>"Click a single match's result to reveal just that one (again to hide)."</li>
                <li>
                    "On a bracket, click " <span class="kbd">"Bracket"</span>
                    " to step through it round by round — names, then scores. Or click a "
                    "round title, or one match, to reveal just that. A later round's teams "
                    "show only once its feeder matches' scores are out, so you can't spoil "
                    "who advanced."
                </li>
            </ul>

            <h2>"Match reminders"</h2>
            <p>
                "Tap a star for a browser notification before a match starts (your browser "
                "asks permission the first time). A filled " <span class="kbd">"★"</span>
                " means you're following it."
            </p>
            <ul>
                <li><span class="kbd">"☆"</span> " on a match — just that match."</li>
                <li>
                    <span class="kbd">"☆"</span>
                    " by a sport tab or in an event's header — every match in that sport or "
                    "event, including ones added later."
                </li>
            </ul>
            <p class="about-note">"(No stars means reminders aren't enabled on this instance.)"</p>

            <h2>"Finding matches"</h2>
            <p>
                "You start with every tier-1 match. Click sport tabs ("
                <span class="kbd">"CS2"</span> " / " <span class="kbd">"LoL"</span>
                ") and event chips to narrow it — the address bar updates so a filtered view "
                "is shareable. Use " <span class="kbd">"‹ show earlier days"</span>
                " or the calendar to look back, and click an event's name for its full "
                "schedule, standings, and bracket."
            </p>

            <p class="about-note">
                "Data is from PandaScore and refreshes in the background; a "
                <span class="kbd">"LIVE"</span>
                " badge is inferred, and scores fill in shortly after a match ends."
            </p>

            <p class="about-back">
                <A href="/">"← back to the schedule"</A>
            </p>
        </article>
    }
}
