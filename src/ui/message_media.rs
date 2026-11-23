use super::State;
use crate::model::{Attachment, Embed};
use reqwest::Url;
use rune::{
    col, el, text,
    tok::{self, type_},
    El,
};

pub(super) fn embed_card(index: usize, embed: &Embed) -> El {
    let t = tok::theme(true);
    let heading = |value: &str| text(value).font(type_::LABEL).color(t.text).wrap_anywhere();
    let body = |value: &str| text(value).font(type_::BODY).color(t.text).wrap_anywhere();
    let mut children = Vec::new();
    if let Some(author) = &embed.author {
        children.push(secondary(&author.name));
    }
    if let Some(title) = embed.title.as_deref() {
        children.push(heading(title));
    }
    if let Some(description) = embed.description.as_deref() {
        children.push(body(description));
    }
    for field in &embed.fields {
        children.push(
            col()
                .w_full()
                .child(heading(&field.name))
                .child(body(&field.value)),
        );
    }
    if let Some(footer) = &embed.footer {
        children.push(secondary(&footer.text));
    }
    let target = embed.url.as_deref().and_then(https_url);
    let image = [&embed.image, &embed.thumbnail]
        .into_iter()
        .flatten()
        .find_map(|image| https_url(&image.url));
    if let Some(image) = image.filter(|image| Some(image) != target.as_ref()) {
        children.push(browser_link("image", "View image", image));
    }
    if let Some(target) = target {
        children.push(browser_link("link", "Open link", target));
    }
    if children.is_empty() {
        children.push(secondary("Embed preview unavailable"));
    }
    let accent = embed
        .color
        .filter(|color| *color <= 0xff_ffff)
        .and_then(|color| rune::color::hex(&format!("#{color:06x}")))
        .unwrap_or(t.card_border);
    let mut card = col()
        .key(format!("embed-{index}"))
        .w_full()
        .gap(6.0)
        .padding(12.0)
        .radius(7.0)
        .fill(t.card)
        .border_sides([0.0, 0.0, 0.0, 3.0], accent);
    for child in children {
        card = card.child(child);
    }
    card
}

pub(super) fn attachment_card(attachment: &Attachment) -> El {
    let t = tok::theme(true);
    let spoiler = attachment.filename.starts_with("SPOILER_");
    let name = if spoiler {
        "Spoiler attachment"
    } else {
        &attachment.filename
    };
    let size = match attachment.size {
        bytes @ 0..=1023 => format!("{bytes} B"),
        bytes @ 1024..=1_048_575 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        bytes => format!("{:.1} MiB", bytes as f64 / 1_048_576.0),
    };
    let meta = match &attachment.content_type {
        Some(kind) if !spoiler => format!("{kind} · {size}"),
        _ => format!("Attachment · {size}"),
    };
    let mut card = col()
        .key(format!("attachment-{}", attachment.id))
        .w_full()
        .gap(4.0)
        .padding(12.0)
        .radius(7.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .child(text(name).font(type_::LABEL).color(t.text).wrap_anywhere())
        .child(secondary(meta));
    if !spoiler {
        if let Some(description) = &attachment.description {
            card = card.child(secondary(description));
        }
    }
    let url = attachment_url(&attachment.url)
        .or_else(|| attachment.proxy_url.as_deref().and_then(attachment_url));
    card.child(match url {
        Some(url) => browser_link(
            "open",
            if spoiler { "Open spoiler" } else { "Open file" },
            url,
        ),
        None => secondary("Attachment link unavailable"),
    })
}

fn secondary(value: impl Into<String>) -> El {
    text(value.into())
        .font(type_::BODY)
        .color(tok::theme(true).text_secondary)
        .wrap_anywhere()
}

fn browser_link(key: &str, action: &str, url: Url) -> El {
    let t = tok::theme(true);
    let label = format!(
        "{action} in browser · {}",
        url.host_str().unwrap_or_default()
    );
    el().key(key)
        .role("link")
        .label(label.clone())
        .focusable(true)
        .w_full()
        .min_h(28.0)
        .py(4.0)
        .radius(4.0)
        .hover(t.hover)
        .on_press_tx(move |s: &mut State, tx| {
            if tx.open_url(url.as_str()).is_err() {
                s.error = "Could not open the link in your browser.".into();
            }
        })
        .child(text(label).font(type_::LABEL).color(t.text).wrap_anywhere())
}

fn https_url(raw: &str) -> Option<Url> {
    if !raw.starts_with("https://")
        || raw.len() > 8192
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    (url.has_host() && url.username().is_empty() && url.password().is_none()).then_some(url)
}

fn attachment_url(raw: &str) -> Option<Url> {
    let url = https_url(raw)?;
    (matches!(
        url.host_str(),
        Some("cdn.discordapp.com" | "media.discordapp.net")
    ) && url.port().is_none()
        && (url.path().starts_with("/attachments/")
            || url.path().starts_with("/ephemeral-attachments/")))
    .then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_links_are_https_and_have_no_credentials() {
        assert!(https_url("https://example.com/a?x=1").is_some());
        for url in [
            "javascript:alert(1)",
            "file:///etc/hosts",
            "http://example.com",
            "https://user:pass@example.com",
            "https://exa\nmple.com",
        ] {
            assert!(https_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn file_links_keep_the_signature_and_require_a_discord_attachment_route() {
        let signed = "https://cdn.discordapp.com/attachments/1/2/notes.txt?ex=1&is=2&hm=3";
        assert_eq!(attachment_url(signed).unwrap().as_str(), signed);
        assert!(attachment_url("https://media.discordapp.net/attachments/1/2/photo.png").is_some());
        for url in [
            "https://cdn.discordapp.com.example.org/attachments/1/2/notes.txt",
            "https://cdn.discordapp.com:8080/attachments/1/2/notes.txt",
            "https://cdn.discordapp.com@evil.test/attachments/1/2/notes.txt",
            "https://cdn.discordapp.com/attachments/../avatars/avatar.png",
        ] {
            assert!(attachment_url(url).is_none(), "{url}");
        }
    }
}
