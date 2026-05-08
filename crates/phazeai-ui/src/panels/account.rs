//! PhazeAI Cloud sign-in via stored credentials (same path as phazeai-cloud).

use crate::app::show_toast;
use crate::components::button::{phaze_button, ButtonVariant};
use crate::domain_state::IdeState;
use crate::util::open_external_url;
use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, label, scroll, text_input, v_stack, Decorators},
    IntoView,
};
use phazeai_cloud::auth::{login_url, CloudCredentials};
use phazeai_cloud::CloudClient;

fn sync_cloud_fields(
    email_sig: RwSignal<String>,
    token_input: RwSignal<String>,
    status: RwSignal<String>,
) {
    let c = CloudCredentials::load();
    email_sig.set(c.email.clone().unwrap_or_default());
    if !c.is_authenticated() {
        token_input.set(String::new());
    }
    let msg = if c.is_authenticated() {
        format!(
            "Signed in{}.",
            c.email
                .as_ref()
                .map(|e| format!(" as {}", e))
                .unwrap_or_default()
        )
    } else {
        "Not signed in to PhazeAI Cloud.".to_string()
    };
    status.set(msg);
}

pub fn account_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let email_sig = create_rw_signal(String::new());
    let token_input = create_rw_signal(String::new());
    let status = create_rw_signal(String::new());
    let (tx_val, rx_val) = std::sync::mpsc::sync_channel::<String>(4);
    let val_sig = create_signal_from_channel(rx_val);

    {
        let toast = state.workbench.status_toast;
        create_effect(move |_| {
            if let Some(msg) = val_sig.get() {
                if let Some(rest) = msg.strip_prefix("ok:") {
                    show_toast(toast, format!("Cloud OK · {}", rest))
                } else if let Some(rest) = msg.strip_prefix("err:") {
                    show_toast(toast, rest.to_string())
                }
            }
        });
    }

    sync_cloud_fields(email_sig, token_input, status);

    let header = container(label(|| "ACCOUNT".to_string()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .font_weight(floem::text::Weight::BOLD)
            .color(p.text_muted)
            .padding_horiz(12.0)
            .padding_vert(8.0)
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full().border_bottom(1.0).border_color(p.border)
    });

    let status_lbl = label(move || status.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(12.0)
            .color(p.text_secondary)
            .padding_horiz(12.0)
            .padding_bottom(8.0)
    });

    let row_reload = phaze_button(
        "Reload from disk",
        ButtonVariant::Secondary,
        theme,
        move || {
            sync_cloud_fields(email_sig, token_input, status);
        },
    );

    let email_field = text_input(email_sig)
        .placeholder("Email (optional, stored in cloud.toml)")
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border)
                .border_radius(4.0)
                .color(p.text_primary)
                .padding_horiz(8.0)
                .padding_vert(6.0)
                .font_size(12.0)
                .margin_bottom(8.0)
        });

    let token_field = text_input(token_input)
        .placeholder("Paste API token (stored in OS keyring)")
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border)
                .border_radius(4.0)
                .color(p.text_primary)
                .padding_horiz(8.0)
                .padding_vert(6.0)
                .font_size(12.0)
        });

    let row_open = {
        let st = state.clone();
        phaze_button(
            "Open sign-in page",
            ButtonVariant::Secondary,
            theme,
            move || {
                let url = login_url();
                if let Err(e) = open_external_url(url) {
                    show_toast(
                        st.workbench.status_toast,
                        format!("Could not open browser: {}", e),
                    );
                }
            },
        )
    };

    let row_save = {
        let st = state.clone();
        phaze_button("Save token", ButtonVariant::Primary, theme, move || {
            let mut c = CloudCredentials::load();
            let em = email_sig.get_untracked().trim().to_string();
            c.email = if em.is_empty() { None } else { Some(em) };
            let tok = token_input.get_untracked().trim().to_string();
            if tok.is_empty() {
                show_toast(st.workbench.status_toast, "Paste a token before saving");
                return;
            }
            c.api_token = Some(tok);
            match c.save() {
                Ok(()) => {
                    token_input.set(String::new());
                    sync_cloud_fields(email_sig, token_input, status);
                    show_toast(st.workbench.status_toast, "Cloud token saved");
                }
                Err(e) => show_toast(st.workbench.status_toast, format!("Save failed: {}", e)),
            }
        })
    };

    let row_validate = {
        let st = state.clone();
        let tx_val = tx_val.clone();
        phaze_button(
            "Validate session",
            ButtonVariant::Secondary,
            theme,
            move || {
                let c = CloudCredentials::load();
                if !c.is_authenticated() {
                    show_toast(st.workbench.status_toast, "No token on file");
                    return;
                }
                let tx = tx_val.clone();
                std::thread::spawn(move || {
                    let res = (|| {
                        let client =
                            CloudClient::new(&c, "phaze-fast").map_err(|e| e.to_string())?;
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| e.to_string())?;
                        rt.block_on(async { client.validate().await.map_err(|e| e.to_string()) })
                    })();
                    let msg = match res {
                        Ok(info) => format!("ok:{} @ {}", info.email, info.tier),
                        Err(e) => format!("err:{}", e),
                    };
                    let _ = tx.send(msg);
                });
            },
        )
    };

    let row_logout = {
        let st = state.clone();
        phaze_button("Sign out", ButtonVariant::Secondary, theme, move || {
            let mut c = CloudCredentials::load();
            match c.logout() {
                Ok(()) => {
                    sync_cloud_fields(email_sig, token_input, status);
                    show_toast(st.workbench.status_toast, "Signed out of PhazeAI Cloud");
                }
                Err(e) => show_toast(st.workbench.status_toast, format!("Logout failed: {}", e)),
            }
        })
    };

    let fine_print = label(|| {
        "API tokens are issued at app.phazeai.com. The IDE never syncs your local project to the cloud unless you explicitly use a cloud-backed model.".to_string()
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.0)
            .color(p.text_muted)
            .padding_horiz(12.0)
            .padding_top(12.0)
    });

    scroll(
        container(
            v_stack((
                header,
                status_lbl,
                container(row_reload).style(|s| s.padding_horiz(12.0).padding_bottom(4.0)),
                container(email_field).style(|s| s.padding_horiz(12.0).width_full()),
                container(token_field).style(|s| s.padding_horiz(12.0).width_full()),
                container(row_open).style(|s| s.padding_horiz(12.0).padding_top(6.0)),
                container(row_save).style(|s| s.padding_horiz(12.0).padding_top(6.0)),
                container(row_validate).style(|s| s.padding_horiz(12.0).padding_top(6.0)),
                container(row_logout).style(|s| s.padding_horiz(12.0).padding_top(6.0)),
                fine_print,
            ))
            .style(|s| s.flex_col().width_full().padding_bottom(24.0)),
        )
        .style(|s| s.width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full().height_full().background(p.glass_bg)
    })
}
