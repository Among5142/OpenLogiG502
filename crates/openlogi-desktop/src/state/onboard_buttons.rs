//! Onboard buttons: the bindings a G-series mouse keeps in its active HID++
//! `0x8100` onboard profile, read from the mouse and rebound one at a time.
//!
//! Nothing is stored in `config.toml`. The mouse runs these bindings itself,
//! on any computer and with OpenLogi closed, so the swr-backed device query
//! the device-read service owns is the only record.
//!
//! A write goes to the mouse in one request whose answer is its read-back:
//! the new action shows at once, then the read-back replaces it, or a refusal
//! is kept for the card to explain and sends the query back to the mouse for
//! the truth. The answer names the connection it was read on and is dropped
//! once the mouse has moved to another one.

use openlogi_core::hid::{DeviceRoute, OnboardAction, OnboardButtons, WriteError};
use tracing::{debug, warn};

use super::device_key::DeviceKey;
use super::events::StateEvents;
use super::load::OnboardButtonsLoad;
use super::{AppState, StateEvent};

/// Why an onboard-button write did not land, kept until another write is
/// sent or one lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnboardButtonRefusal {
    /// The zero-based button the write was for.
    pub(crate) button: u8,
    /// What the agent or mouse answered.
    pub(crate) error: WriteError,
}

impl AppState {
    /// Whether the active device binds its buttons in an onboard profile
    /// (HID++ `0x8100`).
    #[must_use]
    pub fn current_onboard_buttons_supported(&self) -> bool {
        self.current_record()
            .and_then(|record| record.capabilities)
            .is_some_and(|capabilities| capabilities.onboard_buttons)
    }

    /// Whether a write can reach the active device now: it has a connection
    /// and is awake on it.
    #[must_use]
    pub fn current_onboard_buttons_reachable(&self) -> bool {
        self.current_record()
            .is_some_and(|record| record.online && record.route.is_some())
    }

    /// What is known of the active device's onboard buttons.
    #[must_use]
    pub fn current_onboard_buttons_load(&self) -> OnboardButtonsLoad {
        self.current_record()
            .and_then(|record| {
                self.pointer
                    .reads
                    .onboard_buttons_load(&record.device_key())
            })
            .cloned()
            .unwrap_or_default()
    }

    /// Why the active device's last onboard-button write did not land, if it
    /// did not and nothing was written since.
    #[must_use]
    pub(crate) fn current_onboard_button_refusal(&self) -> Option<OnboardButtonRefusal> {
        self.current_record()
            .and_then(|record| self.pointer.reads.button_refusal(&record.device_key()))
            .cloned()
    }

    /// Bind onboard `button` (zero-based) of the mouse `key` to `action`
    /// through the agent. The card shows the action at once; the mouse's
    /// read-back arrives as [`Self::apply_onboard_button_written`].
    ///
    /// No-op unless `key` is the active device, it reports onboard buttons
    /// and is reachable, its last reading has `button` and shows another
    /// binding there, and that reading is on the mouse's current connection.
    pub fn set_onboard_button(
        &mut self,
        key: &DeviceKey,
        button: u8,
        action: OnboardAction,
    ) -> StateEvents {
        let events = self.for_current_device(StateEvent::OnboardButtonsChanged);
        if !self.current_onboard_buttons_supported() {
            return events;
        }
        if !self.is_current_device(key) {
            debug!(key = %key, "onboard-button write for a device that is no longer selected — ignoring it");
            return events;
        }
        if !self.current_onboard_buttons_reachable() {
            debug!(key = %key, "onboard-button write for a mouse that is offline — ignoring it");
            return events;
        }
        let Some(route) = self
            .current_record()
            .and_then(|record| record.route.clone())
        else {
            return events;
        };
        let OnboardButtonsLoad::Ready(buttons) = self.current_onboard_buttons_load() else {
            return events;
        };
        if !needs_write(&buttons, button, &action) {
            debug!(
                button,
                "the button already does this, or the mouse has no such button"
            );
            return events;
        }
        let Some(write) = self
            .pointer
            .reads
            .begin_button_write(key, &route, button, &action)
        else {
            debug!(
                button,
                "the onboard-button reading is not on the mouse's connection yet — ignoring the write"
            );
            return events;
        };
        self.send_ipc(crate::services::ipc::SetOnboardButton {
            route,
            button,
            action,
            key: key.clone(),
            write,
        });
        events
    }

    /// The mouse's answer to the [`Self::set_onboard_button`] write numbered
    /// `write`: show the read-back, with any write sent since still shown, or
    /// keep the refusal for the card and ask the mouse again so the shown
    /// action does not stand in for its own.
    ///
    /// `route` is the connection the write went out on. Only a read still
    /// subscribed on `route` takes the answer; an answer arriving after a
    /// newer write's answer is dropped too.
    pub fn apply_onboard_button_written(
        &mut self,
        key: &DeviceKey,
        route: &DeviceRoute,
        write: u64,
        result: Result<OnboardButtons, WriteError>,
    ) -> StateEvents {
        if let Err(error) = &result {
            warn!(%error, key = %key, "onboard-button write did not land — re-reading the mouse");
        }
        if !self
            .pointer
            .reads
            .finish_button_write(key, route, write, result)
        {
            debug!(key = %key, write, "onboard-button answer superseded, or for a connection no read is on — dropping it");
            return StateEvents::none();
        }
        StateEvent::OnboardButtonsChanged(key.clone()).into()
    }

    /// Re-read `key`'s onboard buttons — the "click to retry" affordance on a
    /// failed read, and the refresh after the mouse switched profiles itself.
    pub(crate) fn refresh_onboard_buttons(&mut self, key: &DeviceKey) -> StateEvents {
        self.pointer.reads.retry_onboard_buttons(key);
        StateEvent::OnboardButtonsChanged(key.clone()).into()
    }

    pub(super) fn load_current_onboard_buttons(&mut self, cx: &mut gpui::Context<Self>) {
        let Some((key, route)) = self
            .current_record()
            .filter(|record| {
                record
                    .capabilities
                    .is_some_and(|capabilities| capabilities.onboard_buttons)
            })
            .and_then(|record| Some((record.device_key(), record.route.clone()?)))
        else {
            return;
        };
        self.pointer
            .reads
            .ensure_onboard_buttons(key, route, self.ipc_sender(), cx);
    }
}

/// Whether binding `button` to `action` changes what the mouse's last
/// reading shows. `false` for a button the reading does not have.
pub(crate) fn needs_write(buttons: &OnboardButtons, button: u8, action: &OnboardAction) -> bool {
    buttons
        .buttons
        .get(usize::from(button))
        .is_some_and(|entry| {
            entry.binding != openlogi_core::hid::OnboardBinding::Action(action.clone())
        })
}
