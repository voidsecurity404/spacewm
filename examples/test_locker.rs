//! A minimal ext-session-lock client for spacewm's e2e tests: locks the session, fills
//! every lock surface with a color, waits, then unlocks.
//!
//! Usage: test_locker RRGGBB SECONDS
//! Exit status: 0 = locked and unlocked, 1 = the compositor refused the lock.

use std::{io::Write, os::fd::AsFd, time::Duration};

use wayland_client::{
    protocol::{wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    manager: Option<ExtSessionLockManagerV1>,
    outputs: Vec<wl_output::WlOutput>,
    surfaces: Vec<(wl_surface::WlSurface, ExtSessionLockSurfaceV1)>,
    color: [u8; 3],
    locked: bool,
    finished: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        app: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => app.compositor = Some(reg.bind(name, version.min(4), qh, ())),
                "wl_shm" => app.shm = Some(reg.bind(name, 1, qh, ())),
                "wl_output" => app.outputs.push(reg.bind(name, version.min(3), qh, ())),
                "ext_session_lock_manager_v1" => app.manager = Some(reg.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => app.locked = true,
            ext_session_lock_v1::Event::Finished => app.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, ()> for App {
    fn event(
        app: &mut Self,
        lock_surface: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let ext_session_lock_surface_v1::Event::Configure { serial, width, height } = event else {
            return;
        };
        lock_surface.ack_configure(serial);
        let Some(shm) = &app.shm else { return };
        let Some((surface, _)) = app.surfaces.iter().find(|(_, l)| l == lock_surface) else {
            return;
        };
        let (w, h) = (width.max(1) as i32, height.max(1) as i32);
        let fd = rustix::fs::memfd_create("test-locker", rustix::fs::MemfdFlags::CLOEXEC)
            .expect("memfd");
        let mut file = std::fs::File::from(fd);
        let [r, g, b] = app.color;
        let pixel = [b, g, r, 255]; // XRGB8888 in memory
        let row: Vec<u8> = pixel.iter().copied().cycle().take(w as usize * 4).collect();
        for _ in 0..h {
            file.write_all(&row).expect("write");
        }
        let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
    }
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_output::WlOutput,
    wl_surface::WlSurface,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    ExtSessionLockManagerV1
);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let hex = args.get(1).map(String::as_str).unwrap_or("00ff00");
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let c = u32::from_str_radix(hex, 16).expect("color RRGGBB");
    let mut app = App { color: [(c >> 16) as u8, (c >> 8) as u8, c as u8], ..App::default() };

    let conn = Connection::connect_to_env().expect("connect");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    queue.roundtrip(&mut app).expect("roundtrip");
    let (Some(manager), Some(compositor)) = (app.manager.clone(), app.compositor.clone()) else {
        eprintln!("no ext_session_lock_manager_v1");
        std::process::exit(2);
    };
    let lock = manager.lock(&qh, ());
    for output in app.outputs.clone() {
        let surface = compositor.create_surface(&qh, ());
        let lock_surface = lock.get_lock_surface(&surface, &output, &qh, ());
        app.surfaces.push((surface, lock_surface));
    }
    while !app.locked && !app.finished {
        queue.blocking_dispatch(&mut app).expect("dispatch");
    }
    if app.finished {
        eprintln!("lock refused");
        std::process::exit(1);
    }
    println!("locked");
    let until = std::time::Instant::now() + Duration::from_secs_f64(secs);
    while std::time::Instant::now() < until {
        queue.roundtrip(&mut app).expect("roundtrip");
        std::thread::sleep(Duration::from_millis(50));
    }
    lock.unlock_and_destroy();
    queue.roundtrip(&mut app).expect("roundtrip");
    println!("unlocked");
}
