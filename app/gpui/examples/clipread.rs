//! Prints the text selection (the clipboard) of a PRIVATE test compositor (never the desktop's), through ext_data_control.
//! It refuses to run unless WAYLAND_DISPLAY names the test compositor. Prints nothing when there is no text on the clipboard.
use std::io::Read;
use wayland_client::{event_created_child, protocol::{wl_registry, wl_seat::WlSeat}, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1}, ext_data_control_manager_v1::ExtDataControlManagerV1, ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
};

#[derive(Default)]
struct S { mgr: Option<ExtDataControlManagerV1>, seat: Option<WlSeat>, offered: Vec<(ExtDataControlOfferV1, Vec<String>)>, chosen: Option<Option<ExtDataControlOfferV1>>, }
impl Dispatch<wl_registry::WlRegistry, ()> for S {
    fn event(s: &mut Self, reg: &wl_registry::WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            match interface.as_str() {
                "ext_data_control_manager_v1" => s.mgr = Some(reg.bind(name, version.min(1), qh, ())),
                "wl_seat" => s.seat = Some(reg.bind(name, version.min(7), qh, ())),
                _ => {}
            }
        }
    }
}
impl Dispatch<WlSeat, ()> for S { fn event(_: &mut Self, _: &WlSeat, _: <WlSeat as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<ExtDataControlManagerV1, ()> for S { fn event(_: &mut Self, _: &ExtDataControlManagerV1, _: <ExtDataControlManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<ExtDataControlDeviceV1, ()> for S {
    fn event(s: &mut Self, _: &ExtDataControlDeviceV1, ev: ext_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            ext_data_control_device_v1::Event::DataOffer { id } => s.offered.push((id, vec![])),
            ext_data_control_device_v1::Event::Selection { id } => s.chosen = Some(id),
            _ => {}
        }
    }
    event_created_child!(S, ExtDataControlDeviceV1, [ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ())]);
}
impl Dispatch<ExtDataControlOfferV1, ()> for S {
    fn event(s: &mut Self, o: &ExtDataControlOfferV1, ev: ext_data_control_offer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = ev { if let Some((_, m)) = s.offered.iter_mut().find(|(x, _)| x == o) { m.push(mime_type); } }
    }
}

fn main() {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    assert!(display == "wayland-selshot", "clipread only talks to the private test compositor (WAYLAND_DISPLAY=wayland-selshot), not {display:?}");
    let conn = Connection::connect_to_env().expect("compositor");
    let mut q = conn.new_event_queue::<S>();
    let qh = q.handle();
    conn.display().get_registry(&qh, ());
    let mut s = S::default();
    q.roundtrip(&mut s).unwrap();
    let (mgr, seat) = (s.mgr.clone().expect("no ext_data_control"), s.seat.clone().expect("no seat"));
    let _dev = mgr.get_data_device(&seat, &qh, ());
    for _ in 0..4 { q.roundtrip(&mut s).unwrap(); if s.chosen.is_some() { break; } }
    let Some(Some(offer)) = s.chosen.clone() else { return };
    let mimes = s.offered.iter().find(|(o, _)| *o == offer).map(|(_, m)| m.clone()).unwrap_or_default();
    let Some(mime) = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"].iter().find(|m| mimes.iter().any(|x| x == *m)) else { return };
    let (mut rd, wr) = { let (r, w) = std::os::unix::net::UnixStream::pair().unwrap(); (r, w) };
    offer.receive(mime.to_string(), std::os::fd::AsFd::as_fd(&wr));
    drop(wr);
    conn.flush().unwrap();
    rd.set_read_timeout(Some(std::time::Duration::from_secs(3))).ok();
    let mut out = String::new();
    let _ = rd.read_to_string(&mut out);
    print!("{out}");
}
