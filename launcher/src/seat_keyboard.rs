//! Keeps cage's seat keyboard set, so an app opened from the launcher always
//! gets a keyboard layout.
//!
//! Cage makes each keyboard device the seat's keyboard when it's added. When
//! that device goes away, wlroots clears it, but the seat still has keyboards
//! (the power button counts). An 8BitDo receiver adds a "Keyboard" interface
//! while its controller is on and drops it when the controller sleeps, which
//! leaves the TV like that. An app that starts then gets a keyboard with no
//! layout, and Chromium-based apps (VacuumTube) crash on the first modifier
//! event, in `xkb_state_update_mask`.
//!
//! The launcher holds one virtual keyboard. Before each app starts,
//! [`SeatKeyboard::rearm`] sends a modifier change through it: cage then makes
//! that keyboard's group the seat keyboard, and cage never removes groups. A
//! new virtual keyboard per launch would also work, but each one leaves a
//! group behind in cage, with two file descriptors.

use std::ffi::CStr;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;

use anyhow::{Context, Result, bail};
use nix::sys::memfd::{MFdFlags, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use xkbcommon_dl::{xkb_context_flags, xkb_keymap_compile_flags, xkb_keymap_format};

/// wl_keyboard's keymap format "xkb_v1".
const KEYMAP_FORMAT_XKB_V1: u32 = 1;
/// The modifier mask for Shift, which is always xkb's modifier 0.
const SHIFT: u32 = 1;

pub struct SeatKeyboard {
    // Closing the connection would remove the virtual keyboard.
    _conn: Connection,
    queue: EventQueue<State>,
    keyboard: ZwpVirtualKeyboardV1,
}

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);

impl SeatKeyboard {
    /// Connects to the compositor (`WAYLAND_DISPLAY`) and sets up the
    /// virtual keyboard with the default layout.
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().context("connecting to the compositor")?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).context("listing the compositor's globals")?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).context("no wl_seat")?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .context("the compositor has no virtual keyboards")?;
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        let keymap = default_keymap()?;
        let bytes = keymap.to_bytes_with_nul();
        let mut file = File::from(
            memfd_create(c"mushtv-keymap", MFdFlags::MFD_CLOEXEC).context("memfd_create")?,
        );
        file.write_all(bytes).context("writing the keymap")?;
        keyboard.keymap(KEYMAP_FORMAT_XKB_V1, file.as_fd(), bytes.len() as u32);
        queue
            .roundtrip(&mut State)
            .context("setting up the virtual keyboard")?;
        Ok(SeatKeyboard {
            _conn: conn,
            queue,
            keyboard,
        })
    }

    /// Makes this keyboard's group the seat keyboard, and returns once the
    /// compositor has done it. The focused window (the launcher) sees Shift
    /// pressed and released, with no key.
    pub fn rearm(&mut self) -> Result<()> {
        self.keyboard.modifiers(SHIFT, 0, 0, 0);
        self.keyboard.modifiers(0, 0, 0, 0);
        self.queue
            .roundtrip(&mut State)
            .context("rearming the seat keyboard")?;
        Ok(())
    }
}

/// The keymap cage gives its keyboards: xkb's defaults, which follow the
/// `XKB_DEFAULT_*` environment variables.
fn default_keymap() -> Result<std::ffi::CString> {
    let Some(xkb) = xkbcommon_dl::xkbcommon_option() else {
        bail!("libxkbcommon not found");
    };
    // SAFETY: plain libxkbcommon calls; every object is checked for null and
    // released once, and the string is copied before it's freed.
    unsafe {
        let context = (xkb.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_FLAGS);
        if context.is_null() {
            bail!("xkb_context_new failed");
        }
        let keymap = (xkb.xkb_keymap_new_from_names)(
            context,
            std::ptr::null(),
            xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
        );
        (xkb.xkb_context_unref)(context);
        if keymap.is_null() {
            bail!("no default keymap");
        }
        let text =
            (xkb.xkb_keymap_get_as_string)(keymap, xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1);
        (xkb.xkb_keymap_unref)(keymap);
        if text.is_null() {
            bail!("xkb_keymap_get_as_string failed");
        }
        let owned = CStr::from_ptr(text).to_owned();
        nix::libc::free(text as *mut nix::libc::c_void);
        Ok(owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::OwnedFd;
    use std::time::{Duration, Instant};
    use wayland_client::WEnum;
    use wayland_client::protocol::wl_keyboard::{self, KeymapFormat, WlKeyboard};

    #[test]
    fn default_keymap_is_a_whole_keymap() {
        let keymap = default_keymap().unwrap();
        let text = keymap.to_str().unwrap();
        assert!(text.starts_with("xkb_keymap {"), "{}", &text[..40]);
        assert!(text.contains("xkb_symbols"));
        assert!(text.contains("Shift"));
    }

    /// What a newly started app sees: does its keyboard get a layout?
    #[derive(Default)]
    struct Probe {
        keymap: bool,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for Probe {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as wayland_client::Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    delegate_noop!(Probe: ignore WlSeat);
    delegate_noop!(Probe: ZwpVirtualKeyboardManagerV1);
    delegate_noop!(Probe: ZwpVirtualKeyboardV1);

    impl Dispatch<WlKeyboard, ()> for Probe {
        fn event(
            state: &mut Self,
            _: &WlKeyboard,
            event: wl_keyboard::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_keyboard::Event::Keymap { format, fd, .. } = event {
                let _: OwnedFd = fd;
                state.keymap = format == WEnum::Value(KeymapFormat::XkbV1);
            }
        }
    }

    fn new_app_gets_a_keymap() -> bool {
        let conn = Connection::connect_to_env().unwrap();
        let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).unwrap();
        seat.get_keyboard(&qh, ());
        let mut probe = Probe::default();
        queue.roundtrip(&mut probe).unwrap();
        probe.keymap
    }

    /// A keyboard is added (cage makes it the seat keyboard) and removed
    /// again, like an 8BitDo receiver's "Keyboard" when its controller sleeps.
    fn add_and_remove_a_keyboard() {
        let conn = Connection::connect_to_env().unwrap();
        let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).unwrap();
        let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        manager.create_virtual_keyboard(&seat, &qh, ());
        queue.roundtrip(&mut Probe::default()).unwrap();
        // Dropping the connection removes the keyboard.
    }

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Needs a compositor with virtual keyboards and no real keyboard:
    /// `scripts/test-seat-keyboard.sh` runs it in a headless cage.
    #[test]
    #[ignore]
    fn rearm_gives_new_apps_a_keymap() {
        let mut seat_keyboard = SeatKeyboard::connect().unwrap();
        add_and_remove_a_keyboard();
        wait_until("a new app gets no keymap (the crash condition)", || {
            !new_app_gets_a_keymap()
        });
        seat_keyboard.rearm().unwrap();
        assert!(new_app_gets_a_keymap());
        // Another keyboard coming and going breaks it again, and the next
        // rearm (before the next app) fixes it again.
        add_and_remove_a_keyboard();
        wait_until("a new app gets no keymap again", || {
            !new_app_gets_a_keymap()
        });
        seat_keyboard.rearm().unwrap();
        assert!(new_app_gets_a_keymap());
    }
}
