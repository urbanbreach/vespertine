//! Demonstrates desktop notifications with delivery results and feedback sounds (PLT-011).
//!
//! The window shows a frame counter that keeps animating while a notification is
//! posted and a sound plays; both run off the UI thread and their results appear
//! when they resolve. Click the notification (or its button) to see the
//! activation event, including the destination and the activation token.
//!
//! Set `VESPERTINE_EXAMPLE_FORCE_UNAVAILABLE=1` to make the notification service
//! and the audio players unavailable (the session bus address and `PATH` point
//! nowhere): the results then read `Unavailable` and nothing else goes wrong.
//! Set `VESPERTINE_EXAMPLE_AUTOEXIT=1` to print the results to stdout and quit
//! once both have arrived.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::time::Instant;

use vespertine::{
    App, Bounds, Context, Div, SharedString, Stateful, SystemNotification,
    SystemNotificationAction, SystemNotificationDelivery, SystemNotificationResponse, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use vespertine_desktop::sound::{SoundOutcome, SoundSource, Sounds};
use vespertine_platform::application;

const NOTIFICATION_TAG: &str = "vespertine-notifications-and-sounds-example";
const SOUND_CATEGORY: &str = "attention";

struct Demo {
    frames: u64,
    started: Instant,
    sounds: Sounds,
    notification: SharedString,
    sound: SharedString,
    activation: SharedString,
    pending: usize,
    auto_exit: bool,
}

impl Demo {
    fn post_notification(&mut self, cx: &mut Context<Self>) {
        let delivery = cx.post_system_notification(SystemNotification {
            tag: NOTIFICATION_TAG.into(),
            title: "Build finished & <ready>".into(),
            body: "Posted by the VESPERTINE notifications example.".into(),
            actions: vec![SystemNotificationAction {
                id: "open".into(),
                label: "Open".into(),
            }],
            destination: Some("example/destination".into()),
        });
        self.notification = "posting...".into();
        self.pending += 1;
        cx.spawn(async move |this, cx| {
            let delivery = delivery.await;
            let text = match &delivery {
                SystemNotificationDelivery::Delivered => "Delivered".to_string(),
                SystemNotificationDelivery::Unavailable => {
                    "Unavailable (no notification service)".to_string()
                }
                SystemNotificationDelivery::Failed(reason) => format!("Failed: {reason}"),
            };
            this.update(cx, |this, cx| {
                println!("notification: {text}");
                this.notification = text.into();
                this.finish_one(cx);
            })
            .ok();
        })
        .detach();
    }

    fn play_sound(&mut self, cx: &mut Context<Self>) {
        let playback = self.sounds.play(SOUND_CATEGORY);
        self.sound = "playing...".into();
        self.pending += 1;
        cx.spawn(async move |this, cx| {
            let outcome = playback.await;
            let text = match &outcome {
                SoundOutcome::Played => "Played".to_string(),
                SoundOutcome::Disabled => "Disabled".to_string(),
                SoundOutcome::Dropped => "Dropped (already playing)".to_string(),
                SoundOutcome::Unavailable(failures) => {
                    let reasons: Vec<String> = failures
                        .iter()
                        .map(|failure| format!("{}: {}", failure.player, failure.reason))
                        .collect();
                    format!("Unavailable ({})", reasons.join("; "))
                }
                SoundOutcome::Failed(reason) => format!("Failed: {reason}"),
            };
            this.update(cx, |this, cx| {
                println!("sound: {text}");
                this.sound = text.into();
                this.finish_one(cx);
            })
            .ok();
        })
        .detach();
    }

    fn finish_one(&mut self, cx: &mut Context<Self>) {
        self.pending -= 1;
        if self.pending == 0 && self.auto_exit {
            // Keep the window up a little longer so the frame counter shows
            // that frames continued after the results arrived.
            let frames_at_results = self.frames;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                this.update(cx, |this, cx| {
                    println!(
                        "frames rendered: {} ({} after the results arrived)",
                        this.frames,
                        this.frames - frames_at_results
                    );
                    cx.quit();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The counter only advances when frames are produced, so it freezes if
        // the UI thread stalls. Requesting the next frame keeps it animating.
        self.frames += 1;
        window.request_animation_frame();

        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_4()
            .p_8()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5))
            .child(
                div()
                    .text_2xl()
                    .child("VESPERTINE notifications and sounds"),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(format!("frame {}", self.frames))
                    .child(format!(
                        "{:.1} s since start",
                        self.started.elapsed().as_secs_f32()
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        button("post", "Post notification")
                            .on_click(cx.listener(|this, _, _, cx| this.post_notification(cx))),
                    )
                    .child(
                        button("play", "Play sound")
                            .on_click(cx.listener(|this, _, _, cx| this.play_sound(cx))),
                    ),
            )
            .child(
                div()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(0x27272a))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(format!("Notification: {}", self.notification))
                    .child(format!("Sound: {}", self.sound))
                    .child(format!("Activation: {}", self.activation)),
            )
    }
}

fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_4()
        .py_2()
        .rounded_md()
        .bg(rgb(0x3f3f46))
        .hover(|style| style.bg(rgb(0x52525b)))
        .active(|style| style.bg(rgb(0x71717a)))
        .cursor_pointer()
        .child(label)
}

/// A short 16-bit mono WAV beep, generated so the example needs no assets.
fn beep_wav() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 22_050;
    const SAMPLES: u32 = SAMPLE_RATE / 5;
    let mut data = Vec::with_capacity(SAMPLES as usize * 2);
    for index in 0..SAMPLES {
        let t = index as f32 / SAMPLE_RATE as f32;
        let envelope = 1.0 - index as f32 / SAMPLES as f32;
        let sample = (t * 880.0 * std::f32::consts::TAU).sin() * envelope * 0.4;
        data.extend_from_slice(&((sample * i16::MAX as f32) as i16).to_le_bytes());
    }
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0")
}

fn main() {
    if env_flag("VESPERTINE_EXAMPLE_FORCE_UNAVAILABLE") {
        // SAFETY: nothing else is running yet; this is the first thing `main` does.
        unsafe {
            // No session bus to connect to, and no audio player on the path.
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus");
            std::env::set_var("PATH", "/nonexistent");
        }
    }
    let auto_exit = env_flag("VESPERTINE_EXAMPLE_AUTOEXIT");

    application().run(move |cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        cx.set_app_identity(
            "dev.vespertine.notifications-and-sounds",
            "VESPERTINE Notifications and Sounds",
        );

        let sounds = Sounds::new();
        sounds.register_category(SOUND_CATEGORY, SoundSource::bytes(beep_wav()), true);

        let view = cx.new(|_| Demo {
            frames: 0,
            started: Instant::now(),
            sounds,
            notification: "not posted yet".into(),
            sound: "not played yet".into(),
            activation: "no activation yet".into(),
            pending: 0,
            auto_exit,
        });
        cx.on_system_notification_response({
            let view = view.clone();
            move |response, cx| {
                let SystemNotificationResponse {
                    tag,
                    action_id,
                    destination,
                    activation_token,
                } = response;
                view.update(cx, |this, cx| {
                    this.activation = format!(
                        "tag '{tag}', action {action_id:?}, destination {destination:?}, token {}",
                        if activation_token.is_some() {
                            "present"
                        } else {
                            "none"
                        }
                    )
                    .into();
                    cx.notify();
                });
            }
        });

        let bounds = Bounds::centered(None, size(px(640.), px(320.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(vespertine::TitlebarOptions {
                    title: Some("Notifications and Sounds".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            {
                let view = view.clone();
                move |_, _| view
            },
        )
        .expect("failed to open notifications and sounds example window");

        // Post a notification and play a sound right away, while frames keep
        // being produced.
        view.update(cx, |this, cx| {
            this.post_notification(cx);
            this.play_sound(cx);
        });
        cx.activate(true);
    });
}
