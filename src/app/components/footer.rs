//! Site footer.
use crate::app::*;

#[component]
pub(crate) fn SiteFooter() -> impl IntoView {
    let site = Resource::new(|| (), |()| async { get_site().await });
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    view! {
        <footer class="footer" id="site-footer">
            // Line 1: about, then the copyright + external links. (The
            // notifications page is reachable from the header clock icon.)
            <div class="footer-line">
                <A href="/about">"about"</A>
                <Suspense>
                    {move || {
                        site.get()
                            .and_then(Result::ok)
                            .map(|s| {
                                let mut items: Vec<AnyView> = Vec::new();
                                // The copyright links out when a URL is configured.
                                if let Some(c) = s.copyright {
                                    let url = s.copyright_url.filter(|u| !u.trim().is_empty());
                                    items.push(match url {
                                        Some(u) => {
                                            view! { <a href=u target="_blank" rel="noreferrer">{c}</a> }
                                                .into_any()
                                        }
                                        None => view! { <span>{c}</span> }.into_any(),
                                    });
                                }
                                for l in s.links {
                                    items.push(
                                        view! {
                                            <a href=l.url target="_blank" rel="noreferrer">{l.label}</a>
                                        }
                                            .into_any(),
                                    );
                                }
                                // `about` always precedes these, so every item gets
                                // a leading separator.
                                items
                                    .into_iter()
                                    .map(|it| {
                                        view! {
                                            <span class="sep">" · "</span>
                                            {it}
                                        }
                                    })
                                    .collect_view()
                            })
                    }}
                </Suspense>
            </div>
            // Line 2: what/where the data is — last, after everything else.
            <div class="footer-line">
                <span>
                    {move || {
                        if traditional.get() {
                            "data via MLB Stats API, NHL, ESPN & Jolpica"
                        } else {
                            "data via PandaScore"
                        }
                    }}
                </span>
            </div>
        </footer>
    }
}

