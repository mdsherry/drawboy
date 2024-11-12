use egui_extras::{Size, StripBuilder};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Instant,
};
use wif::{Shaft, Warp, Weft, Wif};

use eframe::{
    egui::{
        self, menu, Button, Color32, DragValue, Layout, RichText, Stroke, Ui, Vec2, WidgetText,
    },
    Storage,
};

use crate::ewma::Ewma;

mod pedal;

pub struct MyApp {
    row: u32,
    warp: u32,
    average_row_speed: Ewma,
    last_t: Instant,
    wif: Arc<RwLock<Wif>>,
    wif_path: Arc<RwLock<Option<PathBuf>>>,
    timer_paused: bool,
    pedal_pressed: Arc<AtomicBool>,
    mode: OperationMode,
    threading_mode: ThreadingMode,
    threading_batch_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OperationMode {
    Liftplan,
    Treadling,
    Threading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ThreadingMode {
    Continuous,
    Batched,
}

fn save_serialized<T>(storage: &mut dyn Storage, key: &str, value: &T)
where
    T: Serialize,
{
    if let Ok(serialized) = serde_json::to_string(value) {
        storage.set_string(key, serialized);
    }
}

fn load_serialized<T>(storage: Option<&dyn Storage>, key: &str) -> Option<T>
where
    T: for<'a> Deserialize<'a>,
{
    storage.and_then(|storage| {
        storage
            .get_string(key)
            .and_then(|s| serde_json::from_str(&s).ok())
    })
}

impl MyApp {
    pub fn new(fallback_wif: Wif, cc: &eframe::CreationContext) -> Self {
        let pedal_pressed = Arc::new(AtomicBool::new(false));
        pedal::watch_pedal(cc.egui_ctx.clone(), Arc::clone(&pedal_pressed));

        let row = load_serialized(cc.storage, "row");
        let warp = load_serialized(cc.storage, "warp");
        let mode = load_serialized(cc.storage, "mode");
        let wif_path = load_serialized(cc.storage, "wif_path");
        let wif = wif_path.as_deref().and_then(|path| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|contents| wif::parse(&contents).ok())
        });
        let average_row_speed = load_serialized(cc.storage, "average_row_speed");
        let threading_mode = load_serialized(cc.storage, "threading_mode");
        let threading_batch_size = load_serialized(cc.storage, "threading_batch_size");

        Self {
            row: row.unwrap_or(1),
            warp: warp.unwrap_or(1),
            average_row_speed: average_row_speed.unwrap_or_else(|| Ewma::new(0.1)),
            last_t: Instant::now(),
            wif: Arc::new(RwLock::new(wif.unwrap_or(fallback_wif))),
            wif_path: Arc::new(RwLock::new(wif_path)),
            timer_paused: false,
            pedal_pressed,
            mode: mode.unwrap_or(OperationMode::Liftplan),
            threading_mode: threading_mode.unwrap_or(ThreadingMode::Continuous),
            threading_batch_size: threading_batch_size.unwrap_or(8),
        }
    }

    fn row_counter(&mut self, ui: &mut egui::Ui, last_row: u32) {
        ui.horizontal_top(|ui| {
            let drag_widget = match self.mode {
                OperationMode::Liftplan | OperationMode::Treadling => {
                    ui.label("Row ");
                    DragValue::new(&mut self.row)
                }
                OperationMode::Threading => {
                    ui.label("Thread ");
                    DragValue::new(&mut self.warp)
                }
            };
            let drag_widget = drag_widget
                .range(1..=last_row)
                .clamp_existing_to_range(true)
                .update_while_editing(false);
            if ui.add(drag_widget).changed() {
                self.last_t = Instant::now();
            }
            ui.label(format!("/{last_row}"));
        });
    }

    fn control_buttons(&mut self, ui: &mut egui::Ui, pedal_pressed: bool, last_row: u32) {
        let label = if self.threading_mode() {
            "Next thread"
        } else {
            "Next row"
        };
        let next_row = Button::new(label).min_size([64., 64.].into());
        let var = if self.threading_mode() {
            &mut self.warp
        } else {
            &mut self.row
        };
        if ui.add(next_row).clicked() || pedal_pressed {
            *var += 1;
            if *var > last_row {
                *var = 1;
            }
            if !self.timer_paused {
                self.average_row_speed
                    .record(self.last_t.elapsed().as_secs_f32());
            }
            self.last_t = Instant::now();
        }

        let label = if self.mode == OperationMode::Threading {
            "Prev thread"
        } else {
            "Prev row"
        };
        if ui.button(label).clicked() {
            if let Some(new_row) = var.checked_sub(1) {
                if new_row == 0 {
                    *var = last_row;
                } else {
                    *var = new_row;
                }
            } else {
                *var = last_row;
            }
            self.last_t = Instant::now();
        }
    }

    fn menus(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open").clicked() {
                    let ctx = ctx.clone();
                    let wif = self.wif.clone();
                    let wif_path = self.wif_path.clone();
                    std::thread::spawn(move || {
                        if let Some(fname) = rfd::FileDialog::new()
                            .add_filter("WIF", &["wif"])
                            .set_title("Open WIF file")
                            .pick_file()
                        {
                            match std::fs::read_to_string(&fname) {
                                Ok(contents) => match wif::parse(&contents) {
                                    Err(e) => {
                                        eprintln!("Error parsing WIF file: {e}");
                                    }
                                    Ok(parsed) => {
                                        *wif.write().unwrap() = parsed;
                                        *wif_path.write().unwrap() = Some(fname);
                                        ctx.request_repaint();
                                    }
                                },
                                Err(e) => {
                                    eprintln!("Error opening file {}: {e}", fname.display());
                                }
                            }
                        }
                    });
                    ui.close_menu();
                }
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Mode", |ui| {
                if ui
                    .radio_value(&mut self.mode, OperationMode::Liftplan, "Liftplan")
                    .clicked()
                    || ui
                        .radio_value(&mut self.mode, OperationMode::Treadling, "Treadling")
                        .clicked()
                    || ui
                        .radio_value(&mut self.mode, OperationMode::Threading, "Threading")
                        .clicked()
                {
                    ui.close_menu();
                }
            });
        });
    }

    fn timings(&mut self, ui: &mut egui::Ui, last_row: u32) {
        let row = if self.threading_mode() {
            self.warp
        } else {
            self.row
        };

        ui.label(format!(
            "Average time: {:0.1}s",
            self.average_row_speed.value()
        ));
        if self.average_row_speed.value() > 0.1 {
            let eta = ((last_row - row) as f32 * self.average_row_speed.value()) as u64;
            let h = eta / 3600;
            let m = (eta % 3600) / 60;
            let s = eta % 60;
            ui.label(format!("Time estimate:\n{h}h {m:02}m {s:02}s"));
        }
        if ui.button("Reset timer").clicked() {
            self.average_row_speed.reset();
        }
    }

    fn threading_mode(&mut self) -> bool {
        self.mode == OperationMode::Threading
    }

    fn show_liftplan(&mut self, ui: &mut egui::Ui, wif: Wif, shafts: u32, last_row: u32) {
        let lift_plan = wif.liftplan.as_ref();
        let treadling = wif.treadling.as_ref();
        ui.spacing_mut().item_spacing = Vec2::new(3., 3.);
        StripBuilder::new(ui)
            .cell_layout(Layout::centered_and_justified(egui::Direction::LeftToRight))
            .size(Size::exact(20.))
            .size(Size::exact(40.))
            .size(Size::exact(80.))
            .size(Size::exact(40.))
            .size(Size::exact(20.))
            .size(Size::exact(20.))
            .size(Size::exact(20.))
            .size(Size::exact(20.))
            .size(Size::exact(20.))
            .vertical(|mut strip| {
                for offset in [-2, -1, 0, 1, 2, 3, 4, 5, 6] {
                    let row_num = self.row as i32 + offset;
                    if row_num <= 0 || row_num > last_row as i32 {
                        strip.empty();
                        continue;
                    }

                    let row_num = row_num as u32;
                    let row = if self.mode == OperationMode::Liftplan {
                        lift_plan
                            .and_then(|lift_plan| lift_plan.get(&Weft::from(row_num)))
                            .cloned()
                    } else {
                        treadling.and_then(|treadling| {
                            treadling
                                .get(&Weft::from(row_num))
                                .map(|t| t.iter().map(|t| Shaft::from(t.0)).collect())
                        })
                    };
                    if let Some(row) = row {
                        strip.strip(|sb| {
                            sb.size(Size::exact(20.))
                                .sizes(Size::relative(1. / ((shafts + 1) as f32)), shafts as usize)
                                .horizontal(|mut strip| {
                                    let color = wif.weft_color_u8(row_num).unwrap_or_default();
                                    let color = Color32::from_rgb(color[0], color[1], color[2]);
                                    strip.cell(|ui| {
                                        colour_block(ui, color, offset == 0);
                                    });

                                    for shaft in 1..=shafts {
                                        strip.cell(|ui| {
                                            text_block(
                                                ui,
                                                RichText::new(format!("{shaft}")).size(
                                                    if offset == 0 {
                                                        64.
                                                    } else if offset.abs() == 1 {
                                                        32.
                                                    } else {
                                                        16.
                                                    },
                                                ),
                                                offset == 0,
                                                row.contains(&Shaft::from(shaft)),
                                            );
                                        });
                                    }
                                });
                        });
                    } else {
                        strip.empty();
                    }
                }
            });
    }

    fn show_threading(&mut self, ui: &mut egui::Ui, wif: Wif, shaft_count: u32, last_row: u32) {
        ui.spacing_mut().item_spacing = Vec2::new(3., 3.);
        let cols = self.threading_batch_size;
        let range = if self.threading_mode == ThreadingMode::Continuous {
            (self.warp as i32 - 2..).take(cols as usize)
        } else {
            let start = ((self.warp - 1) / self.threading_batch_size * self.threading_batch_size)
                as i32
                + 1;
            (start..).take(cols as usize)
        };
        let threading: Vec<_> = range
            .clone()
            .map(|thread| {
                (
                    thread,
                    if thread <= 0 {
                        None
                    } else {
                        wif.threading
                            .as_ref()
                            .and_then(|threading| threading.get(&(thread as u32).into()))
                    },
                )
            })
            .collect();
        StripBuilder::new(ui)
            .cell_layout(Layout::centered_and_justified(egui::Direction::LeftToRight))
            .sizes(Size::relative(1. / ((cols + 1) as f32)), cols as usize)
            .horizontal(|mut strip| {
                for &(thread, ref shafts) in &threading {
                    if thread <= 0 || thread > last_row as i32 {
                        strip.empty();
                    } else {
                        strip.strip(|sb| {
                            sb.size(Size::exact(16.))
                                .sizes(
                                    Size::relative(1. / ((shaft_count + 1) as f32)),
                                    shaft_count as usize,
                                )
                                .vertical(|mut strip| {
                                    let colour = wif
                                        .warp_color_u8(Warp::from(thread as u32))
                                        .unwrap_or_default();

                                    let colour = Color32::from_rgb(colour[0], colour[1], colour[2]);
                                    strip.cell(|ui| {
                                        colour_block(ui, colour, thread == self.warp as i32)
                                    });

                                    for shaft in 1..=shaft_count {
                                        if let Some(shafts) = shafts {
                                            if shafts.contains(&Shaft::from(shaft)) {
                                                strip.cell(|ui| {
                                                    text_block(
                                                        ui,
                                                        RichText::new(shaft.to_string()).size(18.),
                                                        thread == self.warp as i32,
                                                        true,
                                                    );
                                                });
                                            } else {
                                                strip.empty();
                                            }
                                        } else {
                                            // Placeholder
                                            strip.empty();
                                        }
                                    }
                                });
                        });
                    }
                }
            });
    }
}

impl eframe::App for MyApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        save_serialized(storage, "row", &self.row);
        save_serialized(storage, "warp", &self.warp);
        save_serialized(storage, "mode", &self.mode);
        save_serialized(storage, "wif_path", &self.wif_path);
        save_serialized(storage, "average_row_speed", &self.average_row_speed);
        save_serialized(storage, "threading_mode", &self.threading_mode);
        save_serialized(storage, "threading_batch_size", &self.threading_batch_size);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_zoom_factor(1.5);
        let wif = self.wif.read().unwrap().clone();
        let pedal_pressed = if self.pedal_pressed.load(Ordering::Acquire) {
            self.pedal_pressed.store(false, Ordering::Relaxed);
            true
        } else {
            false
        };

        let last_row = if self.threading_mode() {
            wif.warp.as_ref().map(|wefts| wefts.threads).unwrap_or(1)
        } else {
            wif.weft.as_ref().map(|wefts| wefts.threads).unwrap_or(1)
        };
        let shafts = if self.mode == OperationMode::Liftplan || self.threading_mode() {
            wif.shafts().unwrap_or(4)
        } else {
            wif.treadles().unwrap_or(6)
        };

        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            self.menus(ui, ctx);
        });
        egui::SidePanel::left("left panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(64.);
                    self.row_counter(ui, last_row);
                    if self.threading_mode() {
                        ui.radio_value(
                            &mut self.threading_mode,
                            ThreadingMode::Continuous,
                            "Continuous",
                        );
                        ui.radio_value(&mut self.threading_mode, ThreadingMode::Batched, "Batched");

                        let drag_value = DragValue::new(&mut self.threading_batch_size)
                            .range(1..=25u32)
                            .update_while_editing(false);
                        ui.add(drag_value);
                    }
                    self.control_buttons(ui, pedal_pressed, last_row);
                    self.timings(ui, last_row);

                    if !self.timer_paused && ui.button("Pause timer").clicked() {
                        self.timer_paused = true;
                    }
                    if self.timer_paused && ui.button("Unpause timer").clicked() {
                        self.timer_paused = false;
                        self.last_t = Instant::now();
                    }
                });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Drawboy");

            if let Some(text) = &wif.text {
                if let Some(title) = &text.title {
                    ui.label(format!("Title: {title}"));
                }
                if let Some(author) = &text.author {
                    ui.label(format!("Author: {author}"));
                }
            }

            ui.group(|ui| {
                if self.mode == OperationMode::Threading {
                    self.show_threading(ui, wif, shafts, last_row);
                } else {
                    self.show_liftplan(ui, wif, shafts, last_row);
                }
            });
        });
    }
}

fn colour_block(ui: &mut Ui, colour: Color32, highlight: bool) {
    let stroke_color = if highlight {
        Color32::WHITE
    } else {
        Color32::DARK_GRAY
    };
    let frame = eframe::egui::Frame::none()
        .inner_margin(0.)
        .outer_margin(0.)
        .fill(colour)
        .stroke(Stroke::new(1., stroke_color));

    frame.show(ui, |ui| {
        ui.label(" ");
    });
}

fn text_block(ui: &mut Ui, text: impl Into<WidgetText>, active_row: bool, active_col: bool) {
    let (bg_color, stroke_color) = match (active_row, active_col) {
        (true, true) => (Color32::DARK_GRAY, Color32::WHITE),
        (true, false) | (false, true) => (Color32::BLACK, Color32::DARK_GRAY),
        _ => (Color32::BLACK, Color32::BLACK),
    };
    let frame = eframe::egui::Frame::none()
        .inner_margin(0.)
        .outer_margin(0.)
        .fill(bg_color)
        .stroke(Stroke::new(1., stroke_color));

    frame.show(ui, |ui| {
        ui.style_mut().visuals.override_text_color = Some(stroke_color);
        ui.label(text);
    });
}
