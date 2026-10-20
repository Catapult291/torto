mod model;
#[cfg(test)]
mod tests;
mod transport;

use crate::async_task::{TaskResult, TaskSlot};
use crate::platform::UserEvent;
use crate::preferences::AppLanguage;
use crate::ui::{Icon, dialog_action_button, icon_button, palette};
pub(crate) use model::{Book, Location, OpenContext, Reader, Translation, book_document, clipped};
use model::{Draft, Kind, Payload, character_count, valid_description};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use transport::{Failure, Outcome};

#[derive(Clone)]
enum Work {
    Prepare(OpenContext, bool),
    Send(Draft),
}
pub(crate) enum Output {
    Ready(Draft, bool),
    Sent(Draft, Outcome),
}
pub(crate) type Message = TaskResult<Output>;

#[derive(Default)]
enum Status {
    #[default]
    Editing,
    Preparing,
    Sending,
    Pending,
    Failed(Failure),
    Submitted(transport::Issue),
}

#[derive(Default)]
pub(crate) struct FeedbackFeature {
    visible: bool,
    loaded: bool,
    locked: bool,
    focus_input: bool,
    draft: Option<Draft>,
    editor: Option<(Kind, String)>,
    seed: Option<OpenContext>,
    status: Status,
    retry_at: Option<Instant>,
    task: TaskSlot<Work>,
}

impl FeedbackFeature {
    pub(crate) fn is_open(&self) -> bool {
        self.visible
    }

    pub(crate) fn open(&mut self, seed: OpenContext) {
        self.visible = true;
        if self.task.is_pending()
            || self.locked && self.draft.is_some() && !matches!(self.status, Status::Submitted(_))
        {
            return;
        }
        if matches!(self.status, Status::Submitted(_)) {
            self.editor = None;
            self.draft = None;
        } else if let Some(draft) = self.draft.take() {
            self.editor = Some((draft.payload.kind, draft.payload.description));
        }
        self.locked = false;
        self.retry_at = None;
        self.status = Status::Preparing;
        self.seed = Some(seed.clone());
        self.task.begin(Work::Prepare(seed, !self.loaded));
    }

    pub(crate) fn complete(&mut self, message: Message) {
        if self.task.complete(message.id).is_none() {
            return;
        }
        match message.result {
            Ok(Output::Ready(mut draft, restored)) => {
                if !restored && let Some((kind, description)) = self.editor.take() {
                    draft.payload.kind = kind;
                    draft.payload.description = description;
                }
                self.loaded = true;
                self.locked = restored;
                self.status = if restored {
                    Status::Pending
                } else {
                    Status::Editing
                };
                self.retry_at = remaining_retry(&draft).map(|d| Instant::now() + d);
                self.draft = Some(draft);
                self.focus_input = !restored;
            }
            Ok(Output::Sent(draft, result)) => {
                self.retry_at = remaining_retry(&draft).map(|d| Instant::now() + d);
                self.status = match result {
                    Outcome::Submitted(issue) => Status::Submitted(issue),
                    Outcome::Pending(_) => Status::Pending,
                    Outcome::Failed(failure, _) => {
                        if failure == Failure::Invalid {
                            self.locked = false;
                            self.focus_input = true;
                        }
                        Status::Failed(failure)
                    }
                };
                self.draft = Some(draft);
            }
            Err(error) => {
                self.status = Status::Failed(if error == "configuration" {
                    Failure::Configuration
                } else {
                    Failure::Storage
                })
            }
        }
    }

    fn send(&mut self) {
        if self.task.is_pending() || self.retry_at.is_some_and(|at| at > Instant::now()) {
            return;
        }
        let Some(draft) = &mut self.draft else {
            return;
        };
        if !valid_description(&draft.payload.description) {
            return;
        }
        if !self.locked {
            draft.payload.request_id = uuid::Uuid::new_v4().to_string();
            draft.payload.description = draft.payload.description.trim().to_owned();
        }
        self.locked = true;
        self.status = Status::Sending;
        self.task.begin(Work::Send(draft.clone()));
    }

    pub(crate) fn spawn_pending_tasks(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
    ) {
        let Some(request) = self.task.take_pending() else {
            return;
        };
        let proxy = proxy.clone();
        self.task.attach_worker(runtime.spawn(async move {
            let result = execute(request.payload).await;
            let _ = proxy.send_event(UserEvent::Feedback(TaskResult {
                id: request.id,
                result,
            }));
        }));
    }

    pub(crate) fn overlay(&mut self, ctx: &egui::Context, language: AppLanguage) {
        if !self.visible {
            return;
        }
        let mut send = false;
        let mut close = false;
        let mut prepare = false;
        let response = crate::ui::modal(ctx, egui::Id::new("feedback-modal"), 0.42)
            .frame(egui::Frame::new().fill(palette().surface).stroke(egui::Stroke::new(1.0, palette().border))
                .corner_radius(12).inner_margin(egui::Margin::symmetric(22, 18)))
            .show(ctx, |ui| {
                ui.set_width(520.0_f32.min((crate::ui::overlay_rect(ctx).width() - 48.0).max(240.0)));
                ui.horizontal(|ui| {
                    ui.heading(language.text("反馈与建议", "Feedback"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close |= icon_button(ui, Icon::X).on_hover_text(language.text("关闭", "Close")).clicked();
                    });
                });
                ui.add_space(12.0);
                let submitted = matches!(self.status, Status::Submitted(_));
                if let Some(draft) = &mut self.draft {
                    if let Status::Submitted(issue) = &self.status {
                        ui.label(language.text("反馈已提交，感谢你的帮助。", "Feedback submitted. Thank you."));
                        ui.hyperlink_to(format!("{} #{}", language.text("查看反馈", "View issue"), issue.number), &issue.url);
                    } else {
                        egui::ScrollArea::vertical().id_salt("feedback-body").auto_shrink([false, true]).max_height((crate::ui::overlay_rect(ctx).height() - 220.0).max(120.0)).show(ui, |ui| {
                            let scroll = &ui.spacing().scroll;
                            let gutter = if scroll.floating {
                                (scroll.bar_width + scroll.bar_inner_margin + scroll.bar_outer_margin + 4.0).max(14.0)
                            } else { 6.0 };
                            ui.set_width((ui.available_width() - gutter).max(1.0));
                            ui.add_enabled_ui(!self.locked, |ui| {
                                ui.horizontal(|ui| {
                                    for (kind, label) in [(Kind::Bug, language.text("缺陷", "Bug")), (Kind::Feature, language.text("功能", "Feature"))] {
                                        if crate::settings::choice_button(ui, label, draft.payload.kind == kind).clicked() {
                                            draft.payload.kind = kind;
                                        }
                                    }
                                });
                                ui.add_space(10.0);
                                let hint = if draft.payload.kind == Kind::Bug {
                                    language.text("发生了什么？如何重现？你期望出现什么结果？", "What happened? How can we reproduce it? What did you expect?")
                                } else { language.text("希望增加什么功能？它能解决什么问题？", "What feature would help, and what problem would it solve?") };
                                let editor = description_editor(ui, &mut draft.payload.description, hint);
                                if self.focus_input { editor.request_focus(); self.focus_input = false; }
                            });
                            ui.label(egui::RichText::new(format!("{} · {}/5000", language.text("至少 10 个非空白字符", "At least 10 non-whitespace characters"), character_count(&draft.payload.description))).small().color(palette().muted));
                            if self.locked {
                                ui.label(egui::RichText::new(language.text("已保留本次提交内容，重试时会核对同一份反馈。", "This submission is preserved. Retrying checks the same report.")).small().color(palette().muted));
                            }
                            ui.add_space(8.0);
                            egui::CollapsingHeader::new(language.text("附带信息", "Included information")).show(ui, |ui| show_context(ui, &draft.payload.context, language));
                        });
                    }
                }
                ui.add_space(12.0);
                match &self.status {
                    Status::Preparing | Status::Sending => {
                        ui.horizontal(|ui| {
                            ui.add(crate::ui::LoadingSpinner::new().size(16.0));
                            ui.label(if matches!(self.status, Status::Preparing) { language.text("正在准备反馈…", "Preparing feedback…") } else { language.text("正在提交…", "Submitting…") });
                        });
                    }
                    Status::Pending => { ui.label(language.text("正在核对提交结果，请稍后点击核对结果。", "The submission outcome needs checking. Check again shortly.")); }
                    Status::Failed(failure) => { ui.colored_label(palette().error, failure_text(*failure, language)); }
                    _ => {}
                }
                let wait = self.retry_at.and_then(|at| at.checked_duration_since(Instant::now()));
                if let Some(wait) = wait {
                    ui.label(format!("{} {} {}", language.text("请等待", "Retry in"), wait.as_secs() + 1, language.text("秒后重试", "seconds")));
                    crate::ui::request_repaint_in(ctx, Duration::from_secs(1).min(wait));
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !submitted {
                        let valid = self.draft.as_ref().is_some_and(|d| valid_description(&d.payload.description));
                        let label = if self.locked { language.text("核对结果 / 重试", "Check / Retry") } else { language.text("提交", "Submit") };
                        send = ui.add_enabled_ui(valid && !self.task.is_pending() && wait.is_none(), |ui| dialog_action_button(ui, label, true)).inner.clicked();
                        if self.draft.is_none() && !self.task.is_pending() { prepare = dialog_action_button(ui, language.text("重试", "Retry"), true).clicked(); }
                    }
                    close |= dialog_action_button(ui, if submitted { language.text("完成", "Done") } else { language.text("关闭", "Close") }, false).clicked();
                });
            });
        if close || response.should_close() {
            self.visible = false;
        }
        if send {
            self.send();
        }
        if prepare && let Some(seed) = self.seed.clone() {
            self.open(seed);
        }
    }
}

fn failure_text(failure: Failure, l: AppLanguage) -> &'static str {
    match failure {
        Failure::Network => l.text("无法连接反馈服务，内容已保留，请检查网络后重试。", "Cannot reach the feedback service. Your report is preserved; check your connection and retry."),
        Failure::RateLimited => l.text("提交过于频繁，请稍后重试。", "Too many submissions. Please retry later."),
        Failure::Invalid => l.text("反馈内容不符合要求，请调整后重新提交。", "Please correct your report before submitting again."),
        Failure::Conflict => l.text("提交编号与已保存的反馈不一致，请保留当前内容并联系维护者。", "This submission ID conflicts with an existing report. Keep this draft and contact the maintainer."),
        Failure::Protocol => l.text("暂时无法确认提交结果，内容已保留，请稍后核对。", "The result could not be verified. Your report is preserved; check again later."),
        Failure::Storage => l.text("无法保存反馈草稿，请检查本地存储后重试。", "Could not save or restore the draft. Check local storage and retry."),
        Failure::Configuration => l.text("反馈服务地址配置无效。", "The feedback service address is invalid."),
        Failure::Service => l.text("反馈服务暂时不可用，内容已保留，请稍后重试。", "The feedback service is unavailable. Your report is preserved; retry later."),
    }
}

fn description_editor(ui: &mut egui::Ui, description: &mut String, hint: &str) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
        let line_spacing = ui.spacing().extra_text_line_spacing;
        ui.fonts_mut(|fonts| {
            let mut job = egui::text::LayoutJob::simple(
                text.as_str().to_owned(),
                font.clone(),
                palette().text,
                width,
            );
            for section in &mut job.sections {
                section.format.line_height = Some(fonts.row_height(&font) + line_spacing);
            }
            fonts.layout_job(job)
        })
    };
    ui.add(
        egui::TextEdit::multiline(description)
            .font(egui::TextStyle::Body)
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(10, 8))
            .layouter(&mut layouter)
            .char_limit(10_000)
            .hint_text(hint),
    )
}

fn show_context(ui: &mut egui::Ui, c: &model::Context, l: AppLanguage) {
    if let Some(b) = &c.book {
        context_group(
            ui,
            "feedback-book",
            l.text("当前书籍", "Current book"),
            |ui, width| {
                context_row(ui, width, l.text("原书书名", "Original title"), &b.title);
                if !b.authors.is_empty() {
                    context_row(ui, width, l.text("作者", "Authors"), &b.authors.join(", "));
                }
                context_row(
                    ui,
                    width,
                    l.text("格式", "Format"),
                    &format!(
                        "{} · {}",
                        b.format,
                        if b.layout == "fixed" {
                            l.text("固定版式", "Fixed layout")
                        } else {
                            l.text("流式版式", "Reflowable layout")
                        }
                    ),
                );
                if !b.languages.is_empty() {
                    context_row(
                        ui,
                        width,
                        l.text("原书语言", "Book language"),
                        &b.languages.join(", "),
                    );
                }
            },
        );
    }
    if c.reader.is_some() || c.location.is_some() {
        context_group(
            ui,
            "feedback-reading",
            l.text("阅读状态", "Reading state"),
            |ui, width| {
                if let Some(p) = &c.location {
                    if let Some(chapter) = &p.chapter {
                        context_row(ui, width, l.text("当前章节", "Chapter"), chapter);
                    }
                    if let Some(page) = p.page {
                        context_row(ui, width, l.text("PDF 页码", "PDF page"), &page.to_string());
                    }
                }
                if let Some(r) = &c.reader {
                    context_row(
                        ui,
                        width,
                        l.text("阅读模式", "Mode"),
                        if r.mode == "focus" {
                            l.text("专注模式", "Focus mode")
                        } else {
                            l.text("经典模式", "Classic mode")
                        },
                    );
                    context_row(
                        ui,
                        width,
                        l.text("排版", "Typesetting"),
                        if r.typesetting == "unified" {
                            l.text("统一版式", "Unified typesetting")
                        } else {
                            l.text("原书版式", "Original typesetting")
                        },
                    );
                    if let Some(pdf) = &r.pdf_reflow {
                        context_row(
                            ui,
                            width,
                            l.text("PDF 视图", "PDF view"),
                            match pdf.as_str() {
                                "local" => l.text("本地重排", "Local reflow"),
                                "ai" => l.text("AI 重排", "AI reflow"),
                                _ => l.text("原版", "Original"),
                            },
                        );
                    }
                    context_row(
                        ui,
                        width,
                        l.text("翻译", "Translation"),
                        &if r.translation.enabled {
                            format!(
                                "{} · {}",
                                l.text("已开启", "On"),
                                r.translation.target_language
                            )
                        } else {
                            l.text("已关闭", "Off").into()
                        },
                    );
                }
                if let Some(p) = &c.location {
                    if let Some(document) = &p.document {
                        context_row(ui, width, l.text("书内文档", "Document"), document);
                    }
                    if let Some(anchor) = &p.anchor {
                        context_row(ui, width, l.text("位置标识", "Anchor"), anchor);
                    }
                }
            },
        );
    }
    context_group(
        ui,
        "feedback-environment",
        l.text("运行环境", "Environment"),
        |ui, width| {
            context_row(
                ui,
                width,
                l.text("应用版本", "App version"),
                &format!("Torto {}", c.app.version),
            );
            context_row(
                ui,
                width,
                l.text("操作系统", "OS"),
                &format!("{} {}", c.os.name, c.os.version),
            );
            context_row(ui, width, l.text("系统架构", "Architecture"), &c.os.arch);
            context_row(
                ui,
                width,
                l.text("窗口尺寸", "Window size"),
                &format!("{} × {} px", c.display.width, c.display.height),
            );
            context_row(
                ui,
                width,
                l.text("显示缩放", "Display scale"),
                &format!("{:.0}%", c.display.scale * 100.0),
            );
        },
    );
}

fn context_value_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() - 100.0).max(80.0)
}

fn context_group(ui: &mut egui::Ui, id: &str, title: &str, rows: impl FnOnce(&mut egui::Ui, f32)) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong());
    ui.add_space(4.0);
    let width = context_value_width(ui);
    ui.push_id(id, |ui| rows(ui, width));
    ui.add_space(10.0);
}

fn context_row(ui: &mut egui::Ui, width: f32, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.allocate_ui_with_layout(
            egui::vec2(88.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(88.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(label).color(palette().muted)).truncate(),
                );
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.add(egui::Label::new(value).truncate().selectable(true));
            },
        );
    });
    ui.add_space(2.0);
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn remaining_retry(draft: &Draft) -> Option<Duration> {
    draft
        .retry_until
        .and_then(|at| at.checked_sub(now_seconds()))
        .filter(|s| *s > 0)
        .map(|s| Duration::from_secs(s.min(604_800)))
}

fn draft_path() -> Result<PathBuf, String> {
    crate::smoke::project_dirs()
        .map(|p| p.data_local_dir().join("feedback/draft.json"))
        .ok_or_else(|| "storage".into())
}

fn prepare(mut seed: OpenContext, restore: bool) -> Result<Output, String> {
    if restore {
        let path = draft_path()?;
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() <= 128 * 1024 => {
                let bytes = std::fs::read(path).map_err(|_| "storage")?;
                let draft: Draft = serde_json::from_slice(&bytes).map_err(|_| "storage")?;
                transport::endpoint(&draft.endpoint)?;
                if draft.version != 1
                    || uuid::Uuid::parse_str(&draft.payload.request_id).is_err()
                    || !valid_description(&draft.payload.description)
                {
                    return Err("storage".into());
                }
                return Ok(Output::Ready(draft, true));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("storage".into()),
        }
    }
    seed.context.os = collect_os();
    if let Some(id) = seed.pdf_id
        && let Some(reader) = &mut seed.context.reader
    {
        reader.pdf_reflow = Some(
            match crate::plugins::pdf_native::selected(&id) {
                crate::plugins::pdf_native::TextSource::Native => "local",
                crate::plugins::pdf_native::TextSource::Ocr => "ai",
            }
            .into(),
        );
    }
    let endpoint = std::env::var("TORTO_FEEDBACK_URL")
        .ok()
        .or_else(|| option_env!("TORTO_FEEDBACK_URL").map(str::to_owned))
        .unwrap_or_else(|| transport::DEFAULT_ENDPOINT.into());
    transport::endpoint(&endpoint)?;
    Ok(Output::Ready(
        Draft {
            version: 1,
            endpoint,
            retry_until: None,
            payload: Payload {
                request_id: uuid::Uuid::new_v4().to_string(),
                kind: Kind::Bug,
                description: String::new(),
                context: seed.context,
            },
        },
        false,
    ))
}

fn collect_os() -> model::Os {
    #[cfg(target_os = "windows")]
    let version = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd.exe")
            .args(["/c", "ver"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()
            .and_then(|o| {
                regex::Regex::new(r"\d+\.\d+\.\d+(?:\.\d+)?")
                    .ok()?
                    .find(&String::from_utf8_lossy(&o.stdout))
                    .map(|m| m.as_str().to_owned())
            })
    };
    #[cfg(target_os = "macos")]
    let version = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let version = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("VERSION_ID=")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        });
    model::Os {
        name: match std::env::consts::OS {
            "windows" => "Windows",
            "macos" => "macOS",
            other => other,
        }
        .into(),
        version: clipped(&version.unwrap_or_else(|| "unknown".into()), 100),
        arch: std::env::consts::ARCH.into(),
    }
}

async fn execute(work: Work) -> Result<Output, String> {
    match work {
        Work::Prepare(seed, restore) => tokio::task::spawn_blocking(move || prepare(seed, restore))
            .await
            .map_err(|_| "storage")?,
        Work::Send(mut draft) => {
            let path = tokio::task::spawn_blocking(draft_path)
                .await
                .map_err(|_| "storage")??;
            let saved = draft.clone();
            let saved_path = path.clone();
            tokio::task::spawn_blocking(move || save_draft(&saved_path, &saved))
                .await
                .map_err(|_| "storage")?
                .map_err(|_| "storage")?;
            let result = transport::submit(&draft).await;
            let retry = match &result {
                Outcome::Pending(wait) => Some(*wait),
                Outcome::Failed(_, wait) => *wait,
                _ => None,
            };
            draft.retry_until = retry.map(|wait| now_seconds() + wait.as_secs());
            let saved = draft.clone();
            let submitted = matches!(
                result,
                Outcome::Submitted(_) | Outcome::Failed(Failure::Invalid, _)
            );
            let _ = tokio::task::spawn_blocking(move || {
                if submitted {
                    let _ = std::fs::remove_file(path);
                } else {
                    let _ = save_draft(&path, &saved);
                }
            })
            .await;
            Ok(Output::Sent(draft, result))
        }
    }
}

fn save_draft(path: &std::path::Path, draft: &Draft) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("draft directory"))?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    crate::persistence::write_json_atomic(path, draft)
}
