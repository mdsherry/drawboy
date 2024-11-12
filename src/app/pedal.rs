use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use eframe::egui;

#[cfg(feature = "rpi")]
pub fn watch_pedal(ctx: egui::Context, pedal_pressed: Arc<AtomicBool>) {
    let pedal_pressed = Arc::clone(&pedal_pressed);
    use std::time::Duration;
    std::thread::spawn(move || {
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
            if let Some(interrupt) = pin.poll_interrupt(false, None).expect("Polling failed?") {
                pedal_pressed.store(true, Ordering::Release);
                ctx.request_repaint();
            }
        }
    });
}

#[cfg(not(feature = "rpi"))]
pub fn watch_pedal(_ctx: egui::Context, _pedal_pressed: Arc<AtomicBool>) {}
