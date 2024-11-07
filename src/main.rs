use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::{Duration, Instant},
};
use wif::{parse, Shaft, Warp, Weft, Wif};

use eframe::egui::{self, Button, Color32, DragValue, Grid, Layout, RichText, Stroke};

fn main() -> eframe::Result {
    env_logger::init();
    let w = include_str!(r"houndstooth.wif",);
    let mut wif = parse(&w).unwrap();
    wif.build_or_validate_liftplan().unwrap();

    let options = eframe::NativeOptions {
        viewport: if cfg!(target_arch = "arm") {
            egui::ViewportBuilder::default().with_fullscreen(true)
        } else {
            egui::ViewportBuilder::default().with_inner_size([1024., 600.])
        },
        ..Default::default()
    };

    eframe::run_native(
        "Drawboy",
        options,
        Box::new(|cc| {
            // This gives us image support:
            // egui_extras::install_image_loaders(&cc.egui_ctx);

            Ok(Box::new(MyApp::new(wif, cc)))
        }),
    )
}

struct MyApp {
    row: u32,
    warp: u32,
    average_row_speed: Ewma,
    last_t: Instant,
    wif: Arc<RwLock<Wif>>,
    timer_paused: bool,
    pedal_pressed: Arc<AtomicBool>,
    mode: OperationMode,
    threading_mode: ThreadingMode,
    threading_batch_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationMode {
    Liftplan,
    Treadling,
    Threading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThreadingMode {
    Continuous,
    Batched,
}

impl MyApp {
    pub fn new(wif: Wif, cc: &eframe::CreationContext) -> Self {
        let pedal_pressed = Arc::new(AtomicBool::new(false));
        {
            let ctx = cc.egui_ctx.clone();
            let pedal_pressed = Arc::clone(&pedal_pressed);

            std::thread::spawn(move || {
                #[cfg(feature = "rpi")]
                {
                    use rppal::gpio::Gpio;

                    let gpio = Gpio::new().expect("No GPIO");
                    let mut pin = gpio
                        .get(26)
                        .expect("Could not claim pin")
                        .into_input_pullup();

                    pin.set_interrupt(
                        rppal::gpio::Trigger::FallingEdge,
                        Some(Duration::from_millis(20)),
                    )
                    .expect("Failed to set interrupt");

                    loop {
                        if let Some(interrupt) =
                            pin.poll_interrupt(false, None).expect("Polling failed?")
                        {
                            pedal_pressed.store(true, Ordering::Release);
                            ctx.request_repaint();
                        }
                    }
                }
            });
        }
        Self {
            row: 1,
            warp: 1,
            average_row_speed: Ewma::new(0.1),
            last_t: Instant::now(),
            wif: Arc::new(RwLock::new(wif)),
            timer_paused: false,
            pedal_pressed,
            mode: OperationMode::Liftplan,
            threading_mode: ThreadingMode::Continuous,
            threading_batch_size: 8,
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
        ui.horizontal_top(|ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open").clicked() {
                    let ctx = ctx.clone();
                    let wif = self.wif.clone();
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
                                        ctx.request_repaint();
                                    }
                                },
                                Err(e) => {
                                    eprintln!("Error opening file {}: {e}", fname.display());
                                }
                            }
                        }
                    });
                }
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Mode", |ui| {
                ui.radio_value(&mut self.mode, OperationMode::Liftplan, "Liftplan");
                ui.radio_value(&mut self.mode, OperationMode::Treadling, "Treadling");
                ui.radio_value(&mut self.mode, OperationMode::Threading, "Threading");
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
            self.average_row_speed.value
        ));
        if self.average_row_speed.value > 0.1 {
            let eta = ((last_row - row) as f32 * self.average_row_speed.value) as u64;
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
        ui.set_max_width(40. * shafts as f32);
        ui.columns(shafts as usize + 1, |ui| {
            for shaft in 1u32..=shafts {
                ui[shaft as usize - 1].vertical_centered_justified(|ui| {
                    for offset in [-2, -1, 0, 1, 2] {
                        let mut row_num = self.row as i32 + offset;
                        if row_num <= 0 || row_num > last_row as i32 {
                            row_num = 0;
                        }
                        let highlight_color = if offset == 0 {
                            Color32::WHITE
                        } else {
                            Color32::DARK_GRAY
                        };
                        let dark_color = if offset == 0 {
                            Color32::DARK_GRAY
                        } else {
                            Color32::BLACK
                        };
                        let row_num = row_num as u32;
                        let row = if self.mode == OperationMode::Liftplan {
                            lift_plan
                                .and_then(|lift_plan| lift_plan.get(&Weft::from(row_num)))
                                .cloned()
                        } else {
                            treadling.and_then(|treadling| {
                                treadling
                                    .get(&Weft::from(row_num))
                                    .map(|t| t.into_iter().map(|t| Shaft::from(t.0)).collect())
                            })
                        };

                        let frame = if row.is_some_and(|row| row.contains(&Shaft::from(shaft))) {
                            ui.style_mut().visuals.override_text_color = Some(highlight_color);
                            eframe::egui::Frame::none()
                                .stroke(Stroke::new(1., highlight_color))
                                .fill(dark_color)
                        } else {
                            ui.style_mut().visuals.override_text_color = Some(dark_color);
                            eframe::egui::Frame::none()
                        };
                        frame.show(ui, |ui| {
                            ui.label(RichText::new(format!("{shaft}")).size(if offset == 0 {
                                64.
                            } else if offset.abs() == 1 {
                                32.
                            } else {
                                16.
                            }));
                        });
                        ui.reset_style();
                    }
                });
            }
            ui[shafts as usize].vertical_centered_justified(|ui| {
                for offset in [-2, -1, 0, 1, 2] {
                    let mut row_num = self.row as i32 + offset;
                    if row_num <= 0 || row_num > last_row as i32 {
                        row_num = 0;
                    }
                    let color = wif.weft_color_u8(row_num as u32).unwrap_or_default();
                    let color = if row_num > 0 {
                        Color32::from_rgb(color[0], color[1], color[2])
                    } else {
                        Color32::TRANSPARENT
                    };
                    let frame = eframe::egui::Frame::none().fill(color).stroke(Stroke::new(
                        1.,
                        if row_num > 0 {
                            if offset == 0 {
                                Color32::WHITE
                            } else {
                                Color32::DARK_GRAY
                            }
                        } else {
                            Color32::TRANSPARENT
                        },
                    ));

                    frame.show(ui, |ui| {
                        ui.label(RichText::new(" ").size(if offset == 0 {
                            64.
                        } else if offset.abs() == 1 {
                            32.
                        } else {
                            16.
                        }));
                    });
                }
            })
        });
    }

    fn show_threading(&mut self, ui: &mut egui::Ui, wif: Wif, shafts: u32) {
        let cols = self.threading_batch_size;
        ui.set_max_width(24. * cols as f32);
        Grid::new("threading")
            .num_columns(cols as _)
            .min_col_width(16.)
            .show(ui, |ui| {
                let range = if self.threading_mode == ThreadingMode::Continuous {
                    (self.warp as i32 - 2..).take(cols as usize)
                } else {
                    let start = ((self.warp - 1) / self.threading_batch_size
                        * self.threading_batch_size) as i32
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
                // TODO: Color row
                for thread in range {
                    let (bg_color, stroke_color) = if thread <= 0 {
                        (Color32::TRANSPARENT, Color32::TRANSPARENT)
                    } else {
                        let colour = wif
                            .warp_color_u8(Warp::from(thread as u32))
                            .unwrap_or_default();
                        (
                            Color32::from_rgb(colour[0], colour[1], colour[2]),
                            Color32::WHITE,
                        )
                    };

                    let frame = eframe::egui::Frame::none()
                        .inner_margin(2.)
                        .fill(bg_color)
                        .stroke(Stroke::new(1., stroke_color));

                    frame.show(ui, |ui| {
                        ui.vertical_centered_justified(|ui| {
                            ui.label(RichText::new(" ").size(12.));
                        });
                    });
                }

                ui.end_row();

                for shaft in 1..=shafts {
                    for &(col_num, ref col) in &threading {
                        let highlight_color = if col_num == self.warp as i32 {
                            Color32::WHITE
                        } else {
                            Color32::DARK_GRAY
                        };
                        let dark_color = if col_num == self.warp as i32 {
                            Color32::DARK_GRAY
                        } else {
                            Color32::BLACK
                        };

                        ui.vertical_centered_justified(|ui| {
                            if let Some(shafts) = col {
                                if shafts.contains(&Shaft::from(shaft)) {
                                    let frame = eframe::egui::Frame::none()
                                        .fill(dark_color)
                                        .inner_margin(2.)
                                        .stroke(Stroke::new(1., highlight_color));
                                    frame.show(ui, |ui| {
                                        ui.label(
                                            RichText::new(shaft.to_string())
                                                .size(12.)
                                                .color(highlight_color),
                                        );
                                    });
                                } else {
                                    ui.label(RichText::new(" ").size(12.));
                                }
                            } else {
                                // Placeholder
                                ui.label(RichText::new(" ").size(12.));
                            }
                        });
                    }
                    ui.end_row();
                }
            });
    }
}

struct Ewma {
    alpha: f32,
    value: f32,
    n: usize,
}

impl Ewma {
    pub fn new(alpha: f32) -> Self {
        Self {
            alpha,
            value: 0.,
            n: 0,
        }
    }
    pub fn record(&mut self, measurement: f32) {
        if self.n == 0 {
            self.value = measurement;
        }
        if self.n < 20 {
            let alpha = self.alpha + (1. / (1 + self.n) as f32) * (1. - self.alpha);
            self.value = alpha * measurement + (1. - alpha) * self.value;
        } else {
            self.value = self.alpha * measurement + (1. - self.alpha) * self.value;
        }

        self.n += 1;
    }

    fn reset(&mut self) {
        self.n = 0;
        self.value = 0.;
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_zoom_factor(1.5);
        let wif = self.wif.read().unwrap().clone();
        let pedal_pressed = if self.pedal_pressed.load(Ordering::Acquire) {
            self.pedal_pressed.store(false, Ordering::Relaxed);
            true
        } else {
            false
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            self.menus(ui, ctx);
            ui.heading("Drawboy");

            if let Some(text) = &wif.text {
                if let Some(title) = &text.title {
                    ui.label(format!("Title: {title}"));
                }
                if let Some(author) = &text.author {
                    ui.label(format!("Author: {author}"));
                }
            }

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
                            .range(1..=16u32)
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

                ui.group(|ui| {
                    if self.mode == OperationMode::Threading {
                        self.show_threading(ui, wif, shafts);
                    } else {
                        self.show_liftplan(ui, wif, shafts, last_row);
                    }
                });
            });
            // ui.image(egui::include_image!(
            //     "../../../crates/egui/assets/ferris.png"
            // ));
        });
    }
}
