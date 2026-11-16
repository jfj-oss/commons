//! A camera on Linux through V4L2 (the kernel's own interface, `/dev/video*`), used directly with `ioctl` and `mmap` through
//! `libc` so that nothing but `libc` and a JPEG decoder (`zune-jpeg`, already built for the `image` crate) joins the build.
//! `nokhwa` and the `v4l` crate both pull `bindgen`/`libclang`-based sys crates and a second `bitflags`/`rustix`, which would
//! change the lock file for every other workspace member; this module is about 300 lines instead. (The SDK has no camera
//! capturer, CALLS-PLAN.md section 5.)
//!
//! What it does for one device: open it, ask for MJPEG (what almost every USB camera sends at 720p and 30 fps) or YUYV at the
//! profile's size, set the frame rate, map four buffers, stream. Each picture is decoded (MJPEG to BGRA with zune-jpeg, then
//! libyuv to I420; YUYV directly to I420) and fitted to the profile's ceiling, then handed to the publisher over the capture
//! channel (`capture.rs`), which drops the oldest when the publisher is slow.
//!
//! Ends: the device unplugged (`ENODEV`/`EIO` while streaming) is reported as a system ending; in use by another app,
//! no permission, or nothing usable to read as an error with a plain message.
//!
//! Not run by any automated test (no camera is ever opened by one; there is no loopback device and the tests must not load
//! kernel modules). The struct layouts are asserted against the kernel's sizes below, and the conversions are unit tests.
use super::camera::CameraInfo;
use super::capture::{bgra_to_i420, fit_i420, Ctx};
use livekit::webrtc::video_frame::I420Buffer;
use std::ffi::CString;
use std::os::raw::{c_int, c_ulong, c_void};
use std::time::{Duration, Instant};

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 { a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24 }
const MJPG: u32 = fourcc(b'M', b'J', b'P', b'G');
const YUYV: u32 = fourcc(b'Y', b'U', b'Y', b'V');
const BUF_CAPTURE: u32 = 1;
const MEM_MMAP: u32 = 1;
const CAP_VIDEO_CAPTURE: u32 = 0x1;
const CAP_META_CAPTURE: u32 = 0x0080_0000;
const CAP_STREAMING: u32 = 0x0400_0000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;

#[repr(C)] #[derive(Clone, Copy)] struct Capability { driver: [u8; 16], card: [u8; 32], bus_info: [u8; 32], version: u32, capabilities: u32, device_caps: u32, reserved: [u32; 3] }
#[repr(C)] #[derive(Clone, Copy)] struct FmtDesc { index: u32, type_: u32, flags: u32, description: [u8; 32], pixelformat: u32, mbus_code: u32, reserved: [u32; 3] }
#[repr(C)] #[derive(Clone, Copy)] struct PixFormat { width: u32, height: u32, pixelformat: u32, field: u32, bytesperline: u32, sizeimage: u32, colorspace: u32, priv_: u32, flags: u32, ycbcr_enc: u32, quantization: u32, xfer_func: u32 }
#[repr(C)] #[derive(Clone, Copy)] struct Format { type_: u32, _pad: u32, pix: PixFormat, _rest: [u8; 152] }
#[repr(C)] #[derive(Clone, Copy)] struct ReqBuffers { count: u32, type_: u32, memory: u32, capabilities: u32, flags: u8, reserved: [u8; 3] }
#[repr(C)] #[derive(Clone, Copy)] struct Buffer { index: u32, type_: u32, bytesused: u32, flags: u32, field: u32, _pad0: u32, timestamp: [i64; 2], timecode: [u32; 4], sequence: u32, memory: u32, m: u64, length: u32, reserved2: u32, request_fd: u32, _pad1: u32 }
#[repr(C)] #[derive(Clone, Copy)] struct StreamParm { type_: u32, capability: u32, capturemode: u32, tpf_num: u32, tpf_den: u32, extendedmode: u32, readbuffers: u32, reserved: [u32; 4], _rest: [u8; 160] }
#[repr(C)] #[derive(Clone, Copy)] struct FrmSizeEnum { index: u32, pixel_format: u32, type_: u32, u: [u32; 6], reserved: [u32; 2] }

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<Capability>() == 104);
    assert!(std::mem::size_of::<FmtDesc>() == 64);
    assert!(std::mem::size_of::<Format>() == 208);
    assert!(std::mem::size_of::<ReqBuffers>() == 20);
    assert!(std::mem::size_of::<Buffer>() == 88);
    assert!(std::mem::size_of::<StreamParm>() == 204);
    assert!(std::mem::size_of::<FrmSizeEnum>() == 44);
};

const fn ioc(dir: u32, nr: u32, size: usize) -> c_ulong { ((dir << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | nr) as c_ulong }
const R: u32 = 2; const W: u32 = 1;
const QUERYCAP: c_ulong = ioc(R, 0, std::mem::size_of::<Capability>());
const ENUM_FMT: c_ulong = ioc(R | W, 2, std::mem::size_of::<FmtDesc>());
const S_FMT: c_ulong = ioc(R | W, 5, std::mem::size_of::<Format>());
const REQBUFS: c_ulong = ioc(R | W, 8, std::mem::size_of::<ReqBuffers>());
const QUERYBUF: c_ulong = ioc(R | W, 9, std::mem::size_of::<Buffer>());
const QBUF: c_ulong = ioc(R | W, 15, std::mem::size_of::<Buffer>());
const DQBUF: c_ulong = ioc(R | W, 17, std::mem::size_of::<Buffer>());
const STREAMON: c_ulong = ioc(W, 18, 4);
const STREAMOFF: c_ulong = ioc(W, 19, 4);
const S_PARM: c_ulong = ioc(R | W, 22, std::mem::size_of::<StreamParm>());
const ENUM_FRAMESIZES: c_ulong = ioc(R | W, 74, std::mem::size_of::<FrmSizeEnum>());

fn zeroed<T: Copy>() -> T { unsafe { std::mem::zeroed() } }
fn ioctl<T>(fd: c_int, req: c_ulong, arg: &mut T) -> std::io::Result<()> {
    loop {
        let r = unsafe { libc::ioctl(fd, req as _, arg as *mut T as *mut c_void) };
        if r >= 0 { return Ok(()); }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted { return Err(e); }
    }
}
fn cstr(b: &[u8]) -> String { String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).trim().to_string() }

struct Fd(c_int);
impl Drop for Fd { fn drop(&mut self) { unsafe { libc::close(self.0); } } }
fn open(path: &str) -> std::io::Result<Fd> {
    let c = CString::new(path).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 { Err(std::io::Error::last_os_error()) } else { Ok(Fd(fd)) }
}

/// The capture devices, each once: video capture nodes that stream (not the metadata nodes of the same camera).
pub fn list() -> Vec<CameraInfo> {
    let mut nodes: Vec<(u32, String)> = std::fs::read_dir("/dev").into_iter().flatten().flatten()
        .filter_map(|e| { let n = e.file_name().to_string_lossy().to_string(); let k = n.strip_prefix("video")?.parse::<u32>().ok()?; Some((k, format!("/dev/{n}"))) }).collect();
    nodes.sort();
    let mut out = vec![];
    for (_, path) in nodes {
        let Ok(fd) = open(&path) else { continue };
        let mut cap: Capability = zeroed();
        if ioctl(fd.0, QUERYCAP, &mut cap).is_err() { continue; }
        let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 { cap.device_caps } else { cap.capabilities };
        if caps & CAP_VIDEO_CAPTURE == 0 || caps & CAP_STREAMING == 0 || caps & CAP_META_CAPTURE != 0 { continue; }
        out.push(CameraInfo { id: path, name: cstr(&cap.card) });
    }
    out
}

fn has_format(fd: c_int, want: u32) -> bool {
    for i in 0.. {
        let mut d: FmtDesc = zeroed();
        d.index = i; d.type_ = BUF_CAPTURE;
        if ioctl(fd, ENUM_FMT, &mut d).is_err() { return false; }
        if d.pixelformat == want { return true; }
    }
    false
}

/// Whether the device lists (or can scale to) the wanted size in this format; used only to log what it offers.
fn offers(fd: c_int, pix: u32, w: u32, h: u32) -> bool {
    for i in 0.. {
        let mut s: FrmSizeEnum = zeroed();
        s.index = i; s.pixel_format = pix;
        if ioctl(fd, ENUM_FRAMESIZES, &mut s).is_err() { return i == 0; }   // a driver that cannot list takes any size
        match s.type_ { 1 => if s.u[0] == w && s.u[1] == h { return true; }, _ => if (s.u[0]..=s.u[1]).contains(&w) && (s.u[3]..=s.u[4]).contains(&h) { return true; } }
    }
    false
}

struct Mapped { ptr: *mut c_void, len: usize }
impl Drop for Mapped { fn drop(&mut self) { unsafe { libc::munmap(self.ptr, self.len); } } }

/// Runs until the capture is stopped or the device ends. `want` is the size and frame rate to ask for.
pub fn run(ctx: Ctx, path: String, want: (u32, u32, u32)) {
    if let Err((msg, system)) = stream(&ctx, &path, want) { ctx.ended(msg, system); }
}

fn explain(e: &std::io::Error, what: &str) -> String {
    match e.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => format!("This account is not allowed to use the camera ({what}). On Linux the camera belongs to the \"video\" group."),
        Some(libc::EBUSY) => "The camera is in use by another app. Close it there and try again.".into(),
        Some(libc::ENOENT) | Some(libc::ENODEV) => "The camera is not there any more.".into(),
        _ => format!("The camera could not be used ({what}: {e})."),
    }
}

fn stream(ctx: &Ctx, path: &str, want: (u32, u32, u32)) -> Result<(), (String, bool)> {
    let e = |what: &str| { let w = what.to_string(); move |x: std::io::Error| (explain(&x, &w), false) };
    // a camera opens once: right after a switch the old thread may still hold it for a moment
    let mut fd = None;
    for _ in 0..15 {
        if ctx.stopped() { return Ok(()); }
        match open(path) { Ok(f) => { fd = Some(f); break; } Err(x) if x.raw_os_error() == Some(libc::EBUSY) => std::thread::sleep(Duration::from_millis(100)), Err(x) => return Err((explain(&x, "open"), false)) }
    }
    let fd = fd.ok_or(("The camera is in use by another app. Close it there and try again.".to_string(), false))?;
    let mut cap: Capability = zeroed();
    ioctl(fd.0, QUERYCAP, &mut cap).map_err(e("querycap"))?;
    let pix = if has_format(fd.0, MJPG) { MJPG } else if has_format(fd.0, YUYV) { YUYV } else { return Err(("This camera sends a picture format that is not supported yet (it needs MJPEG or YUYV).".into(), false)); };
    let mut f: Format = zeroed();
    f.type_ = BUF_CAPTURE;
    f.pix = PixFormat { width: want.0, height: want.1, pixelformat: pix, field: 1, bytesperline: 0, sizeimage: 0, colorspace: 0, priv_: 0, flags: 0, ycbcr_enc: 0, quantization: 0, xfer_func: 0 };
    let mut tries = 0;
    loop {
        match ioctl(fd.0, S_FMT, &mut f) {
            Ok(()) => break,
            Err(x) if x.raw_os_error() == Some(libc::EBUSY) && tries < 15 => { tries += 1; std::thread::sleep(Duration::from_millis(100)); if ctx.stopped() { return Ok(()); } }
            Err(x) => return Err((explain(&x, "set format"), false)),
        }
    }
    let (w, h) = (f.pix.width, f.pix.height);
    if f.pix.pixelformat != pix || w == 0 || h == 0 { return Err(("The camera would not give a picture in a format that can be read.".into(), false)); }
    log::info!("camera {path} ({}): {}x{} {}; {}x{} {}", cstr(&cap.card), w, h, if pix == MJPG { "MJPEG" } else { "YUYV" }, want.0, want.1, if offers(fd.0, pix, want.0, want.1) { "asked, offered" } else { "asked, not listed" });
    let stride = if f.pix.bytesperline > 0 { f.pix.bytesperline } else { w * 2 };
    let mut p: StreamParm = zeroed();
    p.type_ = BUF_CAPTURE; p.tpf_num = 1; p.tpf_den = want.2.max(1);
    let _ = ioctl(fd.0, S_PARM, &mut p);   // best effort: some drivers have one fixed rate

    let mut rb: ReqBuffers = zeroed();
    rb.count = 4; rb.type_ = BUF_CAPTURE; rb.memory = MEM_MMAP;
    ioctl(fd.0, REQBUFS, &mut rb).map_err(e("buffers"))?;
    if rb.count < 2 { return Err(("The camera gave too little memory to stream.".into(), false)); }
    let mut maps: Vec<Mapped> = vec![];
    for i in 0..rb.count {
        let mut b: Buffer = zeroed();
        b.index = i; b.type_ = BUF_CAPTURE; b.memory = MEM_MMAP;
        ioctl(fd.0, QUERYBUF, &mut b).map_err(e("buffer"))?;
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), b.length as usize, libc::PROT_READ, libc::MAP_SHARED, fd.0, (b.m as u32) as libc::off_t) };
        if ptr == libc::MAP_FAILED { return Err((explain(&std::io::Error::last_os_error(), "map"), false)); }
        maps.push(Mapped { ptr, len: b.length as usize });
        ioctl(fd.0, QBUF, &mut b).map_err(e("queue"))?;
    }
    let mut ty = BUF_CAPTURE;
    ioctl(fd.0, STREAMON, &mut ty).map_err(e("start"))?;
    let result = (|| -> Result<(), (String, bool)> {
        let mut last_pic = Instant::now();
        let mut bad = 0u32;
        loop {
            if ctx.stopped() { return Ok(()); }
            if ctx.die.load(std::sync::atomic::Ordering::SeqCst) { return Err(("the camera was unplugged (test)".into(), true)); }
            let mut pfd = libc::pollfd { fd: fd.0, events: libc::POLLIN, revents: 0 };
            let n = unsafe { libc::poll(&mut pfd, 1, 200) };
            if n < 0 { let x = std::io::Error::last_os_error(); if x.kind() == std::io::ErrorKind::Interrupted { continue; } return Err((explain(&x, "wait"), false)); }
            if n == 0 { if last_pic.elapsed() > Duration::from_secs(8) { return Err(("The camera is not sending pictures.".into(), false)); } continue; }
            if pfd.revents & (libc::POLLERR | libc::POLLHUP) != 0 { return Err(("The camera was unplugged.".into(), true)); }
            let mut b: Buffer = zeroed();
            b.type_ = BUF_CAPTURE; b.memory = MEM_MMAP;
            match ioctl(fd.0, DQBUF, &mut b) {
                Ok(()) => {}
                Err(x) if x.raw_os_error() == Some(libc::EAGAIN) => continue,
                Err(x) if matches!(x.raw_os_error(), Some(libc::ENODEV) | Some(libc::EIO) | Some(libc::EPIPE)) => return Err(("The camera was unplugged.".into(), true)),
                Err(x) => return Err((explain(&x, "read"), false)),
            }
            last_pic = Instant::now();
            let m = &maps[b.index as usize];
            let data = unsafe { std::slice::from_raw_parts(m.ptr as *const u8, (b.bytesused as usize).min(m.len)) };
            let pic = if pix == MJPG { decode_mjpeg(data, ctx) } else { yuyv_to_i420(data, stride, w, h).map(|b| fit_i420(b, ctx.limits())) };
            match pic {
                Some(i) => { bad = 0; ctx.frame(i); }
                None => { bad += 1; if bad > 60 { return Err(("The camera's pictures could not be read.".into(), false)); } }
            }
            ioctl(fd.0, QBUF, &mut b).map_err(|x| (explain(&x, "requeue"), false))?;
        }
    })();
    let mut ty = BUF_CAPTURE;
    let _ = ioctl(fd.0, STREAMOFF, &mut ty);
    result
}

fn decode_mjpeg(data: &[u8], ctx: &Ctx) -> Option<I420Buffer> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let o = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), o);
    let px = d.decode().ok()?;
    let i = d.info()?;
    let (w, h) = (i.width as u32, i.height as u32);
    bgra_to_i420(&px, w * 4, w, h, ctx.limits())
}

/// Packed YUYV (Y0 U Y1 V) to I420: luma copied, chroma averaged over two rows. `None` when the buffer is short.
pub fn yuyv_to_i420(data: &[u8], stride: u32, w: u32, h: u32) -> Option<I420Buffer> {
    let (cw, ch) = (w & !1, h & !1);
    let stride = stride as usize;
    if cw < 2 || ch < 2 || data.len() < stride * (ch as usize - 1) + cw as usize * 2 { return None; }
    let mut b = I420Buffer::new(cw, ch);
    let (sy, su, sv) = b.strides();
    let (y, u, v) = b.data_mut();
    for row in 0..ch as usize {
        let src = &data[row * stride..row * stride + cw as usize * 2];
        let dst = &mut y[row * sy as usize..row * sy as usize + cw as usize];
        for (x, px) in src.chunks_exact(2).enumerate() { dst[x] = px[0]; }
    }
    for row in 0..(ch / 2) as usize {
        let (a, c) = (&data[2 * row * stride..], &data[(2 * row + 1) * stride..]);
        for x in 0..(cw / 2) as usize {
            let o = x * 4;
            u[row * su as usize + x] = ((a[o + 1] as u16 + c[o + 1] as u16 + 1) / 2) as u8;
            v[row * sv as usize + x] = ((a[o + 3] as u16 + c[o + 3] as u16 + 1) / 2) as u8;
        }
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::webrtc::video_frame::VideoBuffer;
    #[test]
    fn ioctl_numbers_are_the_kernels() {
        assert_eq!(QUERYCAP, 0x8068_5600);
        assert_eq!(ENUM_FMT, 0xC040_5602);
        assert_eq!(S_FMT, 0xC0D0_5605);
        assert_eq!(REQBUFS, 0xC014_5608);
        assert_eq!(QUERYBUF, 0xC058_5609);
        assert_eq!(QBUF, 0xC058_560F);
        assert_eq!(DQBUF, 0xC058_5611);
        assert_eq!(STREAMON, 0x4004_5612);
        assert_eq!(STREAMOFF, 0x4004_5613);
        assert_eq!(S_PARM, 0xC0CC_5616);
        assert_eq!(ENUM_FRAMESIZES, 0xC02C_564A);
        assert_eq!(MJPG, 0x4750_4A4D);
        assert_eq!(YUYV, 0x5659_5559);
    }
    #[test]
    fn yuyv_becomes_i420() {
        // 4x2: left pair Y 10,20 U 100 V 200, right pair Y 30,40 U 110 V 210; second row Y 50.. U 120 V 220 / 130 / 230
        let row0 = [10u8, 100, 20, 200, 30, 110, 40, 210];
        let row1 = [50u8, 120, 60, 220, 70, 130, 80, 230];
        let data: Vec<u8> = row0.iter().chain(row1.iter()).copied().collect();
        let mut b = yuyv_to_i420(&data, 8, 4, 2).unwrap();
        assert_eq!((b.width(), b.height()), (4, 2));
        let (y, u, v) = b.data();
        let sy = b.strides().0 as usize;
        assert_eq!(&y[..4], &[10, 20, 30, 40]);
        assert_eq!(&y[sy..sy + 4], &[50, 60, 70, 80]);
        assert_eq!(&u[..2], &[110, 120]);
        assert_eq!(&v[..2], &[210, 220]);
        let _ = &mut b;
        assert!(yuyv_to_i420(&data[..10], 8, 4, 2).is_none());
    }
}
