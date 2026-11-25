//! The Star Citizen window through Windows Graphics Capture: the same path
//! OBS's window capture takes, so it works while another app is in front
//! and with the game borderless or windowed. A minimised game has no frames
//! to give.
//!
//! Two ways in:
//! - [`capture_game`]: one shot. A capture session is opened, the first
//!   frame copied to the CPU, and everything closed again; nothing keeps
//!   running between shots (an app's idle footprint stays small).
//! - [`Session`]: kept open for reading several times a second. The
//!   device, the capture and the CPU copy are set up once; each call hands
//!   out the newest frame, whole ([`Session::frame`]) or only a rectangle
//!   of it ([`Session::region`]), which is all that leaves the GPU.
//!   Nothing runs between calls: the capture's two-frame pool fills and
//!   Windows drops what comes after, so a call that finds it full empties
//!   it and waits for the next frame (one frame interval of the game, at
//!   most 100 ms when the window has stopped changing) instead of handing
//!   out a frame from just after the previous call.
//!
//! Moved from Starlume's `svc-capture::window`; frames are
//! [`image::RgbaImage`].

use image::RgbaImage as Frame;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    NoWindow,
    Minimised,
    Unsupported,
    Timeout,
    Os(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::NoWindow => {
                write!(f, "Star Citizen isn't running (no game window found)")
            }
            CaptureError::Minimised => {
                write!(f, "Star Citizen is minimised; bring it back to capture it")
            }
            CaptureError::Unsupported => {
                write!(f, "window capture needs Windows 10 (1903) or later")
            }
            CaptureError::Timeout => write!(f, "the game window gave no frame in time"),
            CaptureError::Os(e) => write!(f, "window capture failed: {e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// The game's executable, whose window is captured.
const GAME_EXE: &str = "StarCitizen.exe";

/// Capture the game window now.
pub fn capture_game() -> Result<Frame, CaptureError> {
    Session::open()?.frame()
}

/// A rectangle of a frame and where it sits in the window.
#[derive(Debug, Clone)]
pub struct Region {
    /// Its top-left in window pixels (the asked one, clamped to the window).
    pub x: u32,
    pub y: u32,
    pub image: Frame,
}

/// A capture of one window, kept open between frames. Dropping it ends the
/// capture.
///
/// Calls take `&mut self` (one GPU context); the session may move between
/// threads.
pub struct Session(imp::Session);

impl Session {
    /// Open a capture of the game window.
    pub fn open() -> Result<Self, CaptureError> {
        Self::open_exe(GAME_EXE)
    }

    /// Open a capture of the visible, titled top-level window of the
    /// process running `exe` (a file name such as `notepad.exe`, any
    /// case): tools and tests without the game. A missing window is
    /// [`CaptureError::NoWindow`].
    pub fn open_exe(exe: &str) -> Result<Self, CaptureError> {
        imp::Session::open(exe).map(Self)
    }

    /// The window's size in pixels, as of the newest frame (0 x 0 before
    /// the first).
    pub fn size(&self) -> (u32, u32) {
        self.0.size()
    }

    /// The newest frame's size, to place a [`Session::region`] in. Waits
    /// up to two seconds when no frame has arrived yet, and for the next
    /// frame (up to 100 ms) when the last call was long enough ago that
    /// the frames waiting are old.
    pub fn newest_size(&mut self) -> Result<(u32, u32), CaptureError> {
        self.0.newest()
    }

    /// The newest frame, whole. Waits as [`Session::newest_size`] does.
    pub fn frame(&mut self) -> Result<Frame, CaptureError> {
        let (w, h) = self.0.newest()?;
        self.0.copy(0, 0, w, h)
    }

    /// The rectangle (`x`, `y`, `w`, `h`) of the newest frame, clamped to
    /// the window; only that leaves the GPU. A rectangle entirely outside
    /// the window comes back empty (0 x 0) at the clamped corner.
    pub fn region(&mut self, x: u32, y: u32, w: u32, h: u32) -> Result<Region, CaptureError> {
        let (fw, fh) = self.0.newest()?;
        let (x, y) = (x.min(fw), y.min(fh));
        let (w, h) = (w.min(fw - x), h.min(fh - y));
        let image = if w == 0 || h == 0 {
            Frame::new(0, 0)
        } else {
            self.0.copy(x, y, w, h)?
        };
        Ok(Region { x, y, image })
    }
}

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};

    use windows::Graphics::Capture::{
        Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
        GraphicsCaptureSession,
    };
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Graphics::SizeInt32;
    use windows::Win32::Foundation::{CloseHandle, HMODULE, HWND, LPARAM, MAX_PATH};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindow,
        IsWindowVisible,
    };
    use windows::core::{BOOL, Interface, PWSTR};

    use super::{CaptureError, Frame};

    const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
    /// Frames the pool holds: one being copied, one arriving.
    const BUFFERS: i32 = 2;
    const FIRST_FRAME: Duration = Duration::from_secs(2);
    /// How long a call waits for a frame newer than the ones a full pool
    /// held: a few frames of a game at 30 fps.
    const FRESH: Duration = Duration::from_millis(100);

    fn os(e: windows::core::Error) -> CaptureError {
        CaptureError::Os(e.message().to_string())
    }

    /// The exe name of the process owning `hwnd`.
    fn exe_of(hwnd: HWND) -> Option<String> {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        };
        unsafe {
            let _ = CloseHandle(process);
        }
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_string)
    }

    /// The visible, titled top-level window of the process running `exe`.
    fn window_of(exe: &str) -> Option<HWND> {
        struct Search<'a> {
            exe: &'a str,
            found: Option<HWND>,
        }
        unsafe extern "system" fn each(hwnd: HWND, search: LPARAM) -> BOOL {
            let search = unsafe { &mut *(search.0 as *mut Search) };
            let titled = unsafe { GetWindowTextLengthW(hwnd) } > 0;
            if titled
                && unsafe { IsWindowVisible(hwnd) }.as_bool()
                && exe_of(hwnd).is_some_and(|e| e.eq_ignore_ascii_case(search.exe))
            {
                search.found = Some(hwnd);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut search = Search { exe, found: None };
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut search as *mut _ as isize));
        }
        search.found
    }

    fn texture(
        device: &ID3D11Device,
        desc: &D3D11_TEXTURE2D_DESC,
    ) -> Result<ID3D11Texture2D, CaptureError> {
        let mut t: Option<ID3D11Texture2D> = None;
        unsafe { device.CreateTexture2D(desc, None, Some(&mut t)) }.map_err(os)?;
        t.ok_or_else(|| CaptureError::Os("no texture".into()))
    }

    /// A texture's description with this size, usage and CPU access.
    fn like(
        base: &D3D11_TEXTURE2D_DESC,
        w: u32,
        h: u32,
        usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE,
        cpu: u32,
    ) -> D3D11_TEXTURE2D_DESC {
        D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Usage: usage,
            BindFlags: 0,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
            ..*base
        }
    }

    pub struct Session {
        hwnd: HWND,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        d3d: IDirect3DDevice,
        pool: Direct3D11CaptureFramePool,
        session: GraphicsCaptureSession,
        /// The pool's frame size; recreated when the window's size changes.
        pool_size: SizeInt32,
        /// The newest frame's content, kept on the GPU, and its size.
        latest: Option<(ID3D11Texture2D, u32, u32)>,
        /// The CPU-readable copy, reused while the asked size stays.
        staging: Option<(ID3D11Texture2D, u32, u32)>,
    }

    // SAFETY: the WinRT capture objects are agile (the pool is free-threaded)
    // and the D3D11 device and immediate context may be used from any thread
    // as long as not from two at once, which `&mut self` on every use that
    // touches the context guarantees.
    unsafe impl Send for Session {}

    impl Session {
        pub fn open(exe: &str) -> Result<Self, CaptureError> {
            if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
                return Err(CaptureError::Unsupported);
            }
            let hwnd = window_of(exe).ok_or(CaptureError::NoWindow)?;
            if unsafe { IsIconic(hwnd) }.as_bool() {
                return Err(CaptureError::Minimised);
            }
            let interop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                    .map_err(os)?;
            let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }.map_err(os)?;

            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
            }
            .map_err(os)?;
            let device = device.ok_or_else(|| CaptureError::Os("no D3D11 device".into()))?;
            let context = context.ok_or_else(|| CaptureError::Os("no D3D11 context".into()))?;
            let dxgi: IDXGIDevice = device.cast().map_err(os)?;
            let d3d: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
                .map_err(os)?
                .cast()
                .map_err(os)?;

            let pool_size = item.Size().map_err(os)?;
            let pool =
                Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, FORMAT, BUFFERS, pool_size)
                    .map_err(os)?;
            let session = pool.CreateCaptureSession(&item).map_err(os)?;
            // Windows 11: no yellow capture border, no cursor. Older builds
            // lack these; the capture still works.
            let _ = session.SetIsBorderRequired(false);
            let _ = session.SetIsCursorCaptureEnabled(false);
            session.StartCapture().map_err(os)?;
            Ok(Self {
                hwnd,
                device,
                context,
                d3d,
                pool,
                session,
                pool_size,
                latest: None,
                staging: None,
            })
        }

        pub fn size(&self) -> (u32, u32) {
            self.latest.as_ref().map_or((0, 0), |l| (l.1, l.2))
        }

        /// Take the newest frame into `latest`: the newest waiting in the
        /// pool, or, when the pool was full (it then dropped every frame
        /// since), the next one to arrive. Waits for the first frame if
        /// there is none yet. The newest frame's size.
        pub fn newest(&mut self) -> Result<(u32, u32), CaptureError> {
            if !unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
                return Err(CaptureError::NoWindow);
            }
            if unsafe { IsIconic(self.hwnd) }.as_bool() {
                return Err(CaptureError::Minimised);
            }
            let start = Instant::now();
            loop {
                let (mut newest, waiting) = self.drain();
                // A full pool took no more frames: what it held is from just
                // after the last call, however long ago that was.
                if waiting >= BUFFERS as usize
                    && let Some(stale) = newest.take()
                {
                    newest = Some(self.fresher(stale));
                }
                if let Some(f) = newest {
                    let taken = self.take(&f);
                    let _ = f.Close();
                    taken?;
                }
                if let Some(l) = &self.latest {
                    return Ok((l.1, l.2));
                }
                if start.elapsed() > FIRST_FRAME {
                    return Err(CaptureError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        }

        /// Every frame waiting in the pool: the newest, the others closed,
        /// and how many there were.
        fn drain(&self) -> (Option<Direct3D11CaptureFrame>, usize) {
            let mut newest: Option<Direct3D11CaptureFrame> = None;
            let mut n = 0;
            while let Ok(f) = self.pool.TryGetNextFrame() {
                n += 1;
                if let Some(old) = newest.replace(f) {
                    let _ = old.Close();
                }
            }
            (newest, n)
        }

        /// The next frame to arrive, now that the pool has room again, or
        /// `stale` when none comes within [`FRESH`] (the window's content
        /// isn't changing: `stale` is then how it still looks, unless it
        /// changed while the pool was full and stopped since).
        fn fresher(&self, stale: Direct3D11CaptureFrame) -> Direct3D11CaptureFrame {
            let start = Instant::now();
            while start.elapsed() < FRESH {
                if let (Some(f), _) = self.drain() {
                    let _ = stale.Close();
                    return f;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            stale
        }

        /// Keep `frame`'s content in `latest`; follow a window resize.
        fn take(&mut self, frame: &Direct3D11CaptureFrame) -> Result<(), CaptureError> {
            let content = frame.ContentSize().map_err(os)?;
            let access: IDirect3DDxgiInterfaceAccess =
                frame.Surface().map_err(os)?.cast().map_err(os)?;
            let surface: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(os)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { surface.GetDesc(&mut desc) };
            // The content can be smaller than the surface (the window shrank)
            // or larger (it grew: clipped until the pool follows).
            let w = (content.Width.max(0) as u32).min(desc.Width);
            let h = (content.Height.max(0) as u32).min(desc.Height);
            let resized =
                content.Width != self.pool_size.Width || content.Height != self.pool_size.Height;
            if resized && content.Width > 0 && content.Height > 0 {
                self.pool
                    .Recreate(&self.d3d, FORMAT, BUFFERS, content)
                    .map_err(os)?;
                self.pool_size = content;
            }
            if w == 0 || h == 0 {
                return Ok(());
            }
            if self.latest.as_ref().is_none_or(|l| (l.1, l.2) != (w, h)) {
                let t = texture(&self.device, &like(&desc, w, h, D3D11_USAGE_DEFAULT, 0))?;
                self.latest = Some((t, w, h));
            }
            let (latest, _, _) = self.latest.as_ref().expect("set above");
            let area = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: w,
                bottom: h,
                back: 1,
            };
            unsafe {
                self.context
                    .CopySubresourceRegion(latest, 0, 0, 0, 0, &surface, 0, Some(&area))
            };
            Ok(())
        }

        /// The rectangle of `latest` to the CPU, as RGBA. In bounds (the
        /// caller clamps) and not empty.
        pub fn copy(&mut self, x: u32, y: u32, w: u32, h: u32) -> Result<Frame, CaptureError> {
            let Some((latest, _, _)) = &self.latest else {
                return Err(CaptureError::Timeout);
            };
            if self.staging.as_ref().is_none_or(|s| (s.1, s.2) != (w, h)) {
                let mut base = D3D11_TEXTURE2D_DESC::default();
                unsafe { latest.GetDesc(&mut base) };
                let desc = like(
                    &base,
                    w,
                    h,
                    D3D11_USAGE_STAGING,
                    D3D11_CPU_ACCESS_READ.0 as u32,
                );
                self.staging = Some((texture(&self.device, &desc)?, w, h));
            }
            let (staging, _, _) = self.staging.as_ref().expect("set above");
            let area = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x + w,
                bottom: y + h,
                back: 1,
            };
            unsafe {
                self.context
                    .CopySubresourceRegion(staging, 0, 0, 0, 0, latest, 0, Some(&area))
            };
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                self.context
                    .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            }
            .map_err(os)?;
            let row = (w * 4) as usize;
            let mut rgba = vec![0u8; row * h as usize];
            for (y, out) in rgba.chunks_exact_mut(row).enumerate() {
                let src = unsafe {
                    std::slice::from_raw_parts(
                        (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                        row,
                    )
                };
                for (o, p) in out
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src.as_chunks::<4>().0)
                {
                    *o = [p[2], p[1], p[0], 255];
                }
            }
            unsafe { self.context.Unmap(staging, 0) };
            Frame::from_raw(w, h, rgba)
                .ok_or_else(|| CaptureError::Os("frame buffer size mismatch".into()))
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.session.Close();
            let _ = self.pool.Close();
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{CaptureError, Frame};

    pub struct Session;

    impl Session {
        pub fn open(_exe: &str) -> Result<Self, CaptureError> {
            Err(CaptureError::Unsupported)
        }

        pub fn size(&self) -> (u32, u32) {
            (0, 0)
        }

        pub fn newest(&mut self) -> Result<(u32, u32), CaptureError> {
            Err(CaptureError::Unsupported)
        }

        pub fn copy(&mut self, _x: u32, _y: u32, _w: u32, _h: u32) -> Result<Frame, CaptureError> {
            Err(CaptureError::Unsupported)
        }
    }
}
