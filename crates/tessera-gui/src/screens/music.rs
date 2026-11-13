//! The music page: installed versions on the left, the tracks they ship on the
//! right, and a transport bar under both.

use crate::widgets::*;
use crate::*;

/// A slider event's value; both transport sliders are single-valued.
fn single(value: SliderValue) -> f32 {
    match value {
        SliderValue::Single(value) => value,
        SliderValue::Range(start, _) => start,
    }
}

impl Launcher {
    pub(crate) fn music_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = theme::colors(cx);
        let transport = self.music_transport(window, cx);
        let music = &self.state.music;

        let mut versions = div()
            .id("music-versions")
            .v_flex()
            .gap(px(space::XS))
            .p(px(space::MD))
            .size_full()
            .min_h(px(0.0))
            .overflow_y_scroll();
        if music.versions.is_empty() {
            versions = versions.child(
                div()
                    .p(px(space::LG))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child("没有已安装的版本 —— 先下载一个"),
            );
        }
        for (index, dir) in music.versions.iter().enumerate() {
            let selected = index == music.selected;
            let focused = selected && music.focus == MusicFocus::Versions;
            let id = format!("music-version-{index}");
            let name = dir.clone();
            versions = versions.child(
                div()
                    .id(id)
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(rgb(if focused {
                        colors.selected
                    } else {
                        colors.surface
                    }))
                    .min_h(px(layout::ROW_HEIGHT))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(colors.hover)))
                    .text_size(px(font::BODY))
                    .text_color(rgb(if selected {
                        colors.text_accent
                    } else {
                        colors.text_primary
                    }))
                    .child(name)
                    .on_click(
                        cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.state.select_music_version(index);
                            this.refresh(cx);
                        }),
                    ),
            );
        }

        let mut tracks = div()
            .id("music-tracks")
            .v_flex()
            .gap(px(space::XS))
            .p(px(space::MD))
            .size_full()
            .min_h(px(0.0))
            .overflow_y_scroll();
        let library = music.library.as_ref();
        if let Some(library) = library {
            for (index, track) in library.tracks.iter().enumerate() {
                let selected = index == music.cursor;
                let focused = selected && music.focus == MusicFocus::Tracks;
                let size = discover::human_size(track.size);
                let present = track.present;
                let row_id = format!("music-track-{index}");
                tracks = tracks.child(
                    div()
                        .id(row_id)
                        .h_flex()
                        .items_center()
                        .gap(px(space::MD))
                        .px(px(space::MD))
                        .py(px(space::SM))
                        .rounded(px(radius::SM))
                        .bg(rgb(if focused {
                            colors.selected
                        } else {
                            colors.surface
                        }))
                        .min_h(px(layout::ROW_HEIGHT))
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgb(colors.hover)))
                        .min_h(px(layout::ROW_HEIGHT))
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgb(colors.hover)))
                        .on_click(cx.listener(
                            move |this: &mut Launcher, event: &ClickEvent, _window, cx| {
                                this.state.music.focus = MusicFocus::Tracks;
                                this.state.select_music_track(index);
                                // One click only moves the cursor; a double click is
                                // the same gesture the version list uses to launch.
                                if event.click_count() > 1 {
                                    this.state.music_play_selected();
                                }
                                this.refresh(cx);
                            },
                        ))
                        .child(
                            div()
                                .w(px(layout::INDICATOR_WIDTH))
                                .h(px(layout::INDICATOR_HEIGHT))
                                .rounded(px(radius::PILL))
                                .bg(rgb(if selected {
                                    colors.fill_primary
                                } else {
                                    colors.surface
                                })),
                        )
                        .child(
                            div()
                                .w(px(layout::TRACK_GROUP))
                                .text_size(px(font::CAPTION))
                                .text_color(rgb(colors.text_secondary))
                                .child(track.group.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(font::BODY))
                                .text_color(rgb(if selected {
                                    colors.text_accent
                                } else {
                                    colors.text_primary
                                }))
                                .child(track.title.clone()),
                        )
                        .child(
                            div()
                                .w(px(layout::METADATA_WIDTH))
                                .text_size(px(font::CAPTION))
                                .text_color(rgb(colors.text_secondary))
                                .child(size),
                        )
                        .children((!present).then(|| chip(colors, "缺失", colors.status_danger))),
                );
            }
        } else {
            tracks = tracks.child(
                div()
                    .p(px(space::LG))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child(
                        music
                            .message
                            .clone()
                            .unwrap_or_else(|| "选一个版本".to_string()),
                    ),
            );
        }

        let (count, summary) = match library {
            Some(library) => {
                let missing = library.missing();
                let summary = format!(
                    "{} · {} {} 首 · 索引 {} · {}{}",
                    library.version,
                    library.kind.label(),
                    library.tracks.len(),
                    library.index,
                    discover::human_size(library.total_size()),
                    if missing > 0 {
                        format!(" · {missing} 个文件缺失")
                    } else {
                        String::new()
                    }
                );
                (library.tracks.len(), summary)
            }
            None => (0, "没有可列出的曲目".to_string()),
        };

        let versions_title = format!("版本 ({})", music.versions.len());
        let tracks_title = format!("曲目 ({count})");

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(screen_title(colors, "音乐"))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(rgb(colors.text_secondary))
                            .child(summary),
                    )
                    .child(div().flex_1()),
            )
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap(px(space::MD))
                    .child(
                        div()
                            .v_flex()
                            .w(px(layout::MUSIC_VERSIONS))
                            .h_full()
                            .min_h(px(0.0))
                            .child(panel(colors, &versions_title, versions, true)),
                    )
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .min_h(px(0.0))
                            .child(panel(colors, &tracks_title, tracks, true)),
                    ),
            )
            .child(transport)
    }

    /// The transport bar: the records/background switch on the left, then
    /// play/pause, the position, the volume and the play order.
    fn music_transport(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let (progress, volume) = self.ensure_music_sliders(window, cx);
        let kind = self.state.music.kind;
        let playing = self.state.music.playing;
        let position = music::clock(self.state.music.position);
        let total = self
            .state
            .music
            .duration
            .map(music::clock)
            .unwrap_or_else(|| "--:--".to_string());
        let order = self.state.music.order.label();

        // Writing the clock into the slider is what animates the bar. While the
        // pointer drags it, the drag owns the value.
        let fraction = self.state.music.progress();
        if !self.music_seeking && (fraction - self.music_progress_seen).abs() > 0.0005 {
            self.music_progress_seen = fraction;
            progress.update(cx, |state, cx| {
                state.set_value(fraction, window, cx);
            });
        }

        div()
            .h_flex()
            .items_center()
            .gap(px(space::MD))
            .px(px(space::LG))
            .py(px(space::SM))
            .child(self.music_kind_switch(kind, cx))
            .child(
                div()
                    .id("music-play")
                    .h(px(layout::CONTROL_HEIGHT))
                    .w(px(layout::CONTROL_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .bg(rgb(colors.raised))
                    .text_color(rgb(colors.text_accent))
                    .child(if playing { "⏸" } else { "▶" })
                    .on_click(cx.listener(|this: &mut Launcher, _event, _window, cx| {
                        this.state.music_play_pause();
                        this.refresh(cx);
                    })),
            )
            .child(
                div()
                    .w(px(layout::CLOCK_WIDTH))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(position),
            )
            .child(
                div().flex_1().min_w(px(layout::SLIDER_MIN)).child(
                    Slider::new(&progress)
                        .horizontal()
                        .bg(rgb(colors.fill_primary)),
                ),
            )
            .child(
                div()
                    .w(px(layout::CLOCK_WIDTH))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(total),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child("音量"),
            )
            .child(
                div().w(px(layout::VOLUME_WIDTH)).flex_shrink_0().child(
                    Slider::new(&volume)
                        .horizontal()
                        .bg(rgb(colors.fill_primary)),
                ),
            )
            .child(
                div()
                    .id("music-order")
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .border(px(stroke::THIN))
                    .border_color(rgb(colors.border_default))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_accent))
                    .child(order)
                    .on_click(cx.listener(|this: &mut Launcher, _event, _window, cx| {
                        this.state.music_toggle_order();
                        this.refresh(cx);
                    })),
            )
    }

    /// The 唱片 / 背景音乐 switch, leftmost in the transport bar.
    fn music_kind_switch(&self, active: MusicKind, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let mut switch = div()
            .h_flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(space::XS));
        for kind in [MusicKind::Records, MusicKind::Bgm] {
            let picked = kind == active;
            switch = switch.child(
                div()
                    .id(format!("music-kind-{kind:?}"))
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(rgb(if picked {
                        colors.selected
                    } else {
                        colors.surface
                    }))
                    .min_h(px(layout::ROW_HEIGHT))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(colors.hover)))
                    .border(px(stroke::THIN))
                    .border_color(rgb(if picked {
                        colors.border_focus
                    } else {
                        colors.border_default
                    }))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(if picked {
                        colors.text_accent
                    } else {
                        colors.text_secondary
                    }))
                    .child(kind.label())
                    .on_click(
                        cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.state.set_music_kind(kind);
                            this.refresh(cx);
                        }),
                    ),
            );
        }
        switch
    }

    /// Build the two sliders once, and wire their drags to the transport.
    fn ensure_music_sliders(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<SliderState>, Entity<SliderState>) {
        let _ = window;
        if self.music_progress.is_none() {
            let state = cx.new(|_| SliderState::new().min(0.0).max(1.0).step(0.001));
            self.music_progress_sub = Some(cx.subscribe(
                &state,
                |this: &mut Launcher, _state, event: &SliderEvent, cx| match event {
                    SliderEvent::Change(value) => {
                        // The drag is the authority until it is released.
                        this.music_seeking = true;
                        this.state.music_seek(single(*value));
                        this.refresh(cx);
                    }
                    SliderEvent::Release(value) => {
                        this.state.music_seek(single(*value));
                        this.music_seeking = false;
                        this.music_progress_seen = this.state.music.progress();
                        this.refresh(cx);
                    }
                },
            ));
            self.music_progress = Some(state);
        }
        if self.music_volume.is_none() {
            let start = self.state.music.volume;
            let state = cx.new(|_| {
                SliderState::new()
                    .min(0.0)
                    .max(1.0)
                    .step(0.01)
                    .default_value(start)
            });
            self.music_volume_sub = Some(cx.subscribe(
                &state,
                |this: &mut Launcher, _state, event: &SliderEvent, cx| {
                    let value = match event {
                        SliderEvent::Change(value) | SliderEvent::Release(value) => *value,
                    };
                    this.state.music_set_volume(single(value));
                    this.refresh(cx);
                },
            ));
            self.music_volume = Some(state);
        }
        (
            self.music_progress.clone().expect("just created"),
            self.music_volume.clone().expect("just created"),
        )
    }
}
