use maud::{Markup, html};

use super::{BannerKind, Icon, NavItem, banner, csrf_input, icon, page_header, shell};
use crate::health::Health;
use crate::registries::RegistryLogin;

/// Parameters for [`page`] -- same grouping as `secrets::SecretsPage`.
pub struct RegistriesPage<'a> {
    pub csrf: &'a str,
    pub logins: &'a [RegistryLogin],
    /// `registries::auth_file`, for the page's explanation.
    pub saved_file: &'a str,
    /// Registry and username only -- a rejected password is never echoed.
    pub prefill_registry: &'a str,
    pub prefill_username: &'a str,
    pub notice: Option<Markup>,
    pub error: Option<&'a str>,
    pub health: Health,
}

/// The Registries page: the registries podman holds a login for (never the
/// password), and a form that logs in to one through `registries::login`.
pub fn page(p: RegistriesPage<'_>) -> Markup {
    let RegistriesPage {
        csrf,
        logins,
        saved_file,
        prefill_registry,
        prefill_username,
        notice,
        error,
        health,
    } = p;

    let body = html! {
        (page_header("Registries", html! {}))
        p.page-meta {
            "Logins for pulling private images. sooth saves them to " code { (saved_file) }
            ", where podman finds them for every unit it starts — including after a reboot. "
            "Then reference the image as usual, e.g. " code { "Image=ghcr.io/me/app:1.2" } "."
        }

        @if let Some(msg) = notice { div.banner.banner-success { (msg) } }
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }

        h2 { "Logged in" }
        @if logins.is_empty() {
            p.empty { "No registry logins yet." }
        } @else {
            div.table-wrap {
                table.data-table.registries-table {
                    thead { tr { th { "Registry" } th { "Username" } th { "Stored" } th {} } }
                    tbody {
                        @for l in logins {
                            tr id={"registry-" (l.registry)} {
                                td { code { (l.registry) } }
                                td {
                                    @match &l.username {
                                        Some(u) => { (u) }
                                        None => { span.muted { "—" } }
                                    }
                                }
                                td { (stored_badge(l)) }
                                td.registry-row-actions {
                                    // A GET form, so the browser does the
                                    // query-string encoding of `host:port/path`.
                                    form.inline-form method="get" action="/registries#login" {
                                        input type="hidden" name="registry" value=(l.registry);
                                        @if let Some(u) = &l.username {
                                            input type="hidden" name="username" value=(u);
                                        }
                                        button.btn-icon.btn-icon-sm type="submit"
                                            aria-label={"Update the login for " (l.registry)}
                                            title="Update credentials" {
                                            (icon(Icon::Edit))
                                        }
                                    }
                                    form.inline-form method="post" action="/registries/logout"
                                        onsubmit="return confirm('Remove this login? Units pulling from this registry will fail to pull until you log in again.')" {
                                        (csrf_input(csrf))
                                        input type="hidden" name="registry" value=(l.registry);
                                        button.btn-icon.btn-icon-sm.btn-icon-danger type="submit"
                                            aria-label={"Remove the login for " (l.registry)}
                                            title="Remove login" {
                                            (icon(Icon::Trash))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        h2 #login { "Log in" }
        form.card method="post" action="/registries" autocomplete="off" {
            (csrf_input(csrf))
            div.field {
                label for="registry-host" { "Registry" }
                input.input #registry-host type="text" name="registry" value=(prefill_registry)
                    autocomplete="off" autocapitalize="off" spellcheck="false"
                    placeholder="ghcr.io" required;
                p.field-hint {
                    "A host, optionally with a " code { ":port" } " — or with a "
                    code { "/namespace" } " to use this login only for images under it."
                }
            }
            div.field {
                label for="registry-username" { "Username" }
                input.input #registry-username type="text" name="username" value=(prefill_username)
                    autocomplete="off" autocapitalize="off" spellcheck="false" required;
            }
            div.field {
                label for="registry-password" { "Password or access token" }
                // `new-password` keeps the browser from offering sooth's own
                // saved password here.
                input.input #registry-password type="password" name="password"
                    autocomplete="new-password" required;
            }
            button.btn.btn-primary type="submit" { "Log in" }
        }

        p.field-hint {
            "podman checks the credentials with the registry before saving them. Prefer a "
            "read-only token over your account password: the file is owner-only but not "
            "encrypted. Logging in again to the same registry replaces its credentials. "
            "A plain " code { "podman login" } " from a shell is saved under "
            code { "$XDG_RUNTIME_DIR" } " instead and lasts only until reboot — those show as "
            "\"Until reboot\" above; log in here to keep one."
        }
    };
    shell("Registries", Some(NavItem::Registries), Some(health), body)
}

/// Where a login lives: saved (survives a reboot), session-only (doesn't),
/// or both -- where the session entry is the one podman uses for now.
fn stored_badge(l: &RegistryLogin) -> Markup {
    html! {
        @match (l.saved, l.session) {
            (true, false) => {
                span.badge.badge-running title="Saved — survives a reboot" { "Saved" }
            }
            (false, true) => {
                span.badge.badge-warn
                    title="Only in $XDG_RUNTIME_DIR — gone at reboot. Log in here to save it." {
                    "Until reboot"
                }
            }
            _ => {
                span.badge.badge-running { "Saved" }
                " "
                span.muted title="podman reads the session login first until the next reboot" {
                    "(session login overrides)"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(registry: &str, saved: bool, session: bool) -> RegistryLogin {
        RegistryLogin {
            registry: registry.into(),
            username: Some("alice".into()),
            saved,
            session,
        }
    }

    fn render(logins: &[RegistryLogin]) -> String {
        page(RegistriesPage {
            csrf: "tok",
            logins,
            saved_file: "/home/u/.config/containers/auth.json",
            prefill_registry: "",
            prefill_username: "",
            notice: None,
            error: None,
            health: Health::default(),
        })
        .into_string()
    }

    #[test]
    fn lists_logins_with_update_and_remove() {
        let html = render(&[login("ghcr.io/org", true, false)]);
        assert!(html.contains(r#"id="registry-ghcr.io/org""#));
        assert!(html.contains("alice"));
        assert!(html.contains(r#"action="/registries/logout""#));
        assert!(html.contains(r#"name="registry" value="ghcr.io/org""#));
        assert!(html.contains(r#"name="csrf_token" value="tok""#));
        assert!(html.contains(">Saved<"));
    }

    #[test]
    fn flags_session_only_and_shadowed_logins() {
        let html = render(&[login("a.io", false, true)]);
        assert!(html.contains("Until reboot"));
        let html = render(&[login("b.io", true, true)]);
        assert!(html.contains("session login overrides"));
    }

    #[test]
    fn empty_state_and_password_field_never_prefilled() {
        let html = render(&[]);
        assert!(html.contains("No registry logins yet."));
        assert!(html.contains(r#"type="password" name="password" autocomplete="new-password""#));
        assert!(!html.contains(r#"name="password" value"#));
    }
}
