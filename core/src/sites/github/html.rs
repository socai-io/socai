use dom_query::Document;
use reqwest::Url;
use serde_json::{json, Value};

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn absolute(base: &Url, href: &str) -> Option<String> {
    base.join(href)
        .ok()
        .filter(|u| u.scheme() == "https")
        .map(|u| u.to_string())
}

/// Common discovery pages return small records; other pages retain text and links.
/// Selectors were checked against public GitHub pages, not authenticated UI state.
pub fn extract(url: &Url, source: &str) -> Value {
    let document = Document::from(source);
    document
        .select("script, style, svg, template, noscript")
        .remove();
    let title = clean(&document.select("title").text());
    let developers = url.path().starts_with("/trending/developers");
    let trending = url.path().starts_with("/trending");
    let topic = url.path().starts_with("/topics/");
    let mut items = Vec::new();
    if trending || topic {
        let cards = document.select(if trending {
            "article.Box-row"
        } else {
            "article.border"
        });
        for card in cards.iter() {
            let selector = if developers {
                "h1 a"
            } else if topic {
                "h3 a"
            } else {
                "h2 a"
            };
            let mut identity = None;
            for anchor in card.select(selector).iter() {
                if let Some(href) = anchor.attr("href") {
                    let segments = href.trim_matches('/').split('/').count();
                    if segments == if developers { 1 } else { 2 } {
                        identity = Some((href.to_string(), clean(&anchor.text())));
                        break;
                    }
                }
            }
            let Some((href, name)) = identity else {
                continue;
            };
            if developers {
                let repo = card.select("h1 a").iter().find_map(|a| {
                    a.attr("href")
                        .filter(|h| h.trim_matches('/').split('/').count() == 2)
                        .and_then(|h| absolute(url, &h))
                });
                items.push(json!({"login":href.trim_matches('/'),"name":name,
                    "url":absolute(url,&href),
                    "popular_repository":repo,
                    "description":clean(&card.select("article .f6.color-fg-muted.mt-1").text())}));
            } else {
                let topics: Vec<_> = card
                    .select("a.topic-tag")
                    .iter()
                    .map(|a| clean(&a.text()))
                    .collect();
                let builders: Vec<_> = card
                    .select("a[data-hovercard-type='user']")
                    .iter()
                    .filter_map(|a| a.attr("href").map(|h| h.trim_matches('/').to_owned()))
                    .collect();
                items.push(
                    json!({"full_name":href.trim_matches('/'),"url":absolute(url,&href),
                    "description":clean(&card.select("p").text()),
                    "language":clean(&card.select("[itemprop='programmingLanguage']").text()),
                    "stars_text":clean(&card.select("a[href$='/stargazers'], .Counter").text()),
                    "growth_text":clean(&card.select("span.float-sm-right").text()),
                    "topics":topics,"contributors":builders}),
                );
            }
        }
        if !items.is_empty() {
            let next = document
                .select("a[rel='next']")
                .attr("href")
                .and_then(|h| absolute(url, &h))
                .or_else(|| {
                    // Topics use a GET "Load more" form rather than a next link.
                    let form = document.select("form.ajax-pagination-form[method='get']");
                    let action = form.attr("action")?;
                    let mut next = url.join(&action).ok()?;
                    for field in form.select("input[name][value]").iter() {
                        next.query_pairs_mut()
                            .append_pair(&field.attr("name")?, &field.attr("value")?);
                    }
                    Some(next.to_string())
                });
            return json!({"kind":if developers {"trending_developers"} else if trending {"trending"} else {"topic"},
                "title":title,"period":url.query_pairs().find(|(k,_)| k=="since").map(|(_,v)|v.into_owned()).or_else(||trending.then(||"daily".to_string())),
                "items":items,"next_url":next});
        }
    }
    let main = if document.select("main").exists() {
        document.select("main")
    } else {
        document.select("body")
    };
    let mut links = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for a in main.select("a[href]").iter() {
        if let Some(href) = a.attr("href").and_then(|h| absolute(url, &h)) {
            if seen.insert(href.clone()) {
                links.push(json!({"title":clean(&a.text()),"url":href}));
            }
        }
    }
    json!({"kind":"html_text","title":title,"text":clean(&main.text()),"links":links,
        "coverage":"Static public HTML only; dynamic content may be missing. Use REST/GraphQL for complete structured detail.",
        "warning":if trending || topic {Some("Discovery cards not found; page may be empty, blocked or changed. Do not interpret as zero results.")} else {None}})
}
