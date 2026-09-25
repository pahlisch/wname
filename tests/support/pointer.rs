//! A virtual pointer, so tests can move the real cursor the way a mouse
//! does. `hyprctl dispatch movecursor` warps without moving focus, so it
//! cannot tell whether a surface swallows the pointer.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Move the pointer to (x, y) in a layout of the given size, in a few steps
/// so the compositor sees real motion.
pub fn move_to(from: (u32, u32), to: (u32, u32), extent: (u32, u32)) {
    let conn = Connection::connect_to_env().expect("connect to nested compositor");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .expect("zwlr_virtual_pointer_manager_v1");
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    let steps = 8u32;
    for i in 0..=steps {
        let lerp = |a: u32, b: u32| {
            (i64::from(a) + (i64::from(b) - i64::from(a)) * i64::from(i) / i64::from(steps)) as u32
        };
        pointer.motion_absolute(
            i * 10,
            lerp(from.0, to.0),
            lerp(from.1, to.1),
            extent.0,
            extent.1,
        );
        pointer.frame();
        queue.roundtrip(&mut State).expect("roundtrip");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    pointer.destroy();
    manager.destroy();
    queue.roundtrip(&mut State).expect("roundtrip");
}
