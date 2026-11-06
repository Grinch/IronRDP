use ironrdp::pdu::rdp::client_info::PerformanceFlags;

use self::ffi::PerformanceFlagsType;

/// The bitmap settings used when the caller sets none, matching the native client.
///
/// Without a codec on offer, the server falls back to sending the screen as nearly raw bitmaps,
/// which is so much data that it starts dropping frames. Offering RemoteFX fixes that.
fn default_bitmap_config() -> Result<ironrdp::connector::BitmapConfig, Box<crate::error::ffi::IronRdpError>> {
    let codecs = ironrdp::pdu::rdp::capability_sets::client_codecs_capabilities(&[])
        .map_err(|help| -> Box<crate::error::ffi::IronRdpError> { help.as_str().into() })?;

    Ok(ironrdp::connector::BitmapConfig {
        color_depth: 32,
        lossy_compression: true,
        codecs,
    })
}

/// Client Monitor Data and the desktop size covering its monitors, from monitors given as `TS_MONITOR_DEF`s.
///
/// [MS-RDPBCGR] 2.2.1.3.6 "Client Monitor Data (TS_UD_CS_MONITOR)" allows at most 16 monitors, puts the upper-left
/// corner of the primary monitor at (0, 0), and bounds the virtual desktop, the union of the monitors, to between
/// 200 x 200 and 32766 x 32766 pixels.
/// The desktop size sent in Client Core Data is the size of that union, as the native client sends it.
///
/// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
fn client_monitor_layout(
    monitors: &[ironrdp::pdu::gcc::Monitor],
) -> Result<(ironrdp::pdu::gcc::ClientMonitorData, ironrdp::connector::DesktopSize), &'static str> {
    use ironrdp::pdu::gcc::{ClientMonitorData, MONITOR_COUNT_MAX, MonitorFlags};

    const MIN_DESKTOP_EXTENT: i64 = 200;
    const MAX_DESKTOP_EXTENT: i64 = 32_766;

    if monitors.is_empty() {
        return Err("no monitor was added");
    }
    if MONITOR_COUNT_MAX < monitors.len() {
        return Err("more than 16 monitors were added");
    }
    if monitors
        .iter()
        .any(|monitor| monitor.right < monitor.left || monitor.bottom < monitor.top)
    {
        return Err("a monitor has its right edge left of its left edge or its bottom edge above its top edge");
    }
    let mut primary = monitors
        .iter()
        .filter(|monitor| monitor.flags.contains(MonitorFlags::PRIMARY));
    let (Some(primary_monitor), None) = (primary.next(), primary.next()) else {
        return Err("exactly one monitor must be primary");
    };
    if primary_monitor.left != 0 || primary_monitor.top != 0 {
        return Err("the primary monitor must have its upper-left corner at (0, 0)");
    }

    let left = monitors
        .iter()
        .map(|monitor| i64::from(monitor.left))
        .min()
        .unwrap_or(0);
    let top = monitors.iter().map(|monitor| i64::from(monitor.top)).min().unwrap_or(0);
    let right = monitors
        .iter()
        .map(|monitor| i64::from(monitor.right))
        .max()
        .unwrap_or(0);
    let bottom = monitors
        .iter()
        .map(|monitor| i64::from(monitor.bottom))
        .max()
        .unwrap_or(0);
    // The bounds are inclusive.
    let width = right - left + 1;
    let height = bottom - top + 1;
    if !((MIN_DESKTOP_EXTENT..=MAX_DESKTOP_EXTENT).contains(&width)
        && (MIN_DESKTOP_EXTENT..=MAX_DESKTOP_EXTENT).contains(&height))
    {
        return Err("the monitors must span between 200 x 200 and 32766 x 32766 pixels");
    }
    let desktop_size = ironrdp::connector::DesktopSize {
        width: u16::try_from(width).map_err(|_| "desktop width out of range")?,
        height: u16::try_from(height).map_err(|_| "desktop height out of range")?,
    };

    Ok((
        ClientMonitorData {
            monitors: monitors.to_vec(),
        },
        desktop_size,
    ))
}

/// Client Monitor Extended Data ([MS-RDPBCGR] 2.2.1.3.9) giving each monitor the same scale and no physical size,
/// which the server then ignores.
///
/// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
fn monitor_extended_layout(
    monitors: &[ironrdp::pdu::gcc::Monitor],
    desktop_scale_factor: u32,
    device_scale_factor: u32,
) -> ironrdp::pdu::gcc::ClientMonitorExtendedData {
    use ironrdp::pdu::gcc::{ClientMonitorExtendedData, ExtendedMonitorInfo, MonitorOrientation};

    ClientMonitorExtendedData {
        extended_monitors_info: monitors
            .iter()
            .map(|monitor| ExtendedMonitorInfo {
                physical_width: 0,
                physical_height: 0,
                orientation: if monitor.bottom - monitor.top > monitor.right - monitor.left {
                    MonitorOrientation::Portrait
                } else {
                    MonitorOrientation::Landscape
                },
                desktop_scale_factor,
                device_scale_factor,
            })
            .collect(),
    }
}

// `ironrdp::pdu::gcc::KeyboardType` is a permissive newtype (MS-RDPBCGR's value table for this
// field has grown across revisions), not a fieldless enum, so `diplomat::enum_convert` no longer
// applies to it; this conversion is written by hand instead. It must live outside the
// `#[diplomat::bridge]` module below, since that macro only understands inherent impls on the
// bridged types, not trait impls. The FFI-facing enum keeps its existing fixed set of variants
// unchanged.
impl From<ffi::KeyboardType> for ironrdp::pdu::gcc::KeyboardType {
    fn from(value: ffi::KeyboardType) -> Self {
        match value {
            ffi::KeyboardType::IbmPcXt => Self::IBM_PC_XT,
            ffi::KeyboardType::OlivettiIco => Self::OLIVETTI_ICO,
            ffi::KeyboardType::IbmPcAt => Self::IBM_PC_AT,
            ffi::KeyboardType::IbmEnhanced => Self::IBM_ENHANCED,
            ffi::KeyboardType::Nokia1050 => Self::NOKIA_1050,
            ffi::KeyboardType::Nokia9140 => Self::NOKIA_9140,
            ffi::KeyboardType::Japanese => Self::JAPANESE,
        }
    }
}

#[diplomat::bridge]
pub mod ffi {
    use ironrdp::connector::Credentials;
    use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;

    use crate::dvc::ffi::DvcPipeProxyConfig;
    use crate::error::ffi::IronRdpError;

    #[diplomat::opaque]
    pub struct Config {
        pub connector: ironrdp::connector::Config,
        pub dvc_pipe_proxy: Option<DvcPipeProxyConfig>,
    }

    impl Config {
        pub fn get_builder() -> Box<ConfigBuilder> {
            Box::<ConfigBuilder>::default()
        }

        #[diplomat::attr(auto, getter = "dvc_pipe_proxy")]
        pub fn get_dvc_pipe_proxy(&self) -> Option<Box<DvcPipeProxyConfig>> {
            self.dvc_pipe_proxy.as_ref().map(|dvc| Box::new(dvc.clone()))
        }
    }

    #[derive(Default)]
    #[diplomat::opaque_mut]
    pub struct ConfigBuilder {
        pub credentials: Option<Credentials>,
        pub domain: Option<String>,
        pub enable_tls: Option<bool>,
        pub enable_credssp: Option<bool>,
        pub keyboard_type: Option<ironrdp::pdu::gcc::KeyboardType>,
        pub keyboard_subtype: Option<u32>,
        pub keyboard_layout: Option<u32>,
        pub keyboard_functional_keys_count: Option<u32>,
        pub ime_file_name: Option<String>,
        pub dig_product_id: Option<String>,
        pub desktop_size: Option<ironrdp::connector::DesktopSize>,
        pub bitmap: Option<ironrdp::connector::BitmapConfig>,
        pub client_build: Option<u32>,
        pub client_name: Option<String>,
        pub client_dir: Option<String>,
        pub platform: Option<MajorPlatformType>,
        pub enable_server_pointer: Option<bool>,
        pub autologon: Option<bool>,
        pub no_audio_playback: Option<bool>,
        pub pointer_software_rendering: Option<bool>,
        pub performance_flags: Option<ironrdp::pdu::rdp::client_info::PerformanceFlags>,
        pub timezone_info: Option<ironrdp::pdu::rdp::client_info::TimezoneInfo>,
        pub dvc_pipe_proxy: Option<DvcPipeProxyConfig>,
        pub monitors: Vec<ironrdp::pdu::gcc::Monitor>,
        pub desktop_scale_factor: Option<u32>,
    }

    pub enum KeyboardType {
        IbmPcXt,
        OlivettiIco,
        IbmPcAt,
        IbmEnhanced,
        Nokia1050,
        Nokia9140,
        Japanese,
    }

    #[diplomat::opaque]
    pub struct DesktopSize(pub ironrdp::connector::DesktopSize);

    impl DesktopSize {
        #[diplomat::attr(auto, getter = "width")]
        pub fn get_width(&self) -> u16 {
            self.0.width
        }

        #[diplomat::attr(auto, getter = "height")]
        pub fn get_height(&self) -> u16 {
            self.0.height
        }
    }

    impl ConfigBuilder {
        pub fn new() -> Box<Self> {
            Box::<ConfigBuilder>::default()
        }

        pub fn with_username_and_password(&mut self, username: &str, password: &str) {
            self.credentials = Some(Credentials::UsernamePassword {
                username: username.to_owned(),
                password: password.to_owned(),
            });
        }

        #[diplomat::attr(auto, setter = "domain")]
        pub fn set_domain(&mut self, domain: &str) {
            self.domain = Some(domain.to_owned());
        }

        #[diplomat::attr(auto, setter = "enable_tls")]
        pub fn set_enable_tls(&mut self, enable_tls: bool) {
            self.enable_tls = Some(enable_tls);
        }

        #[diplomat::attr(auto, setter = "enable_credssp")]
        pub fn set_enable_credssp(&mut self, enable_credssp: bool) {
            self.enable_credssp = Some(enable_credssp);
        }

        #[diplomat::attr(auto, setter = "keyboard_layout")]
        pub fn set_keyboard_layout(&mut self, keyboard_layout: u32) {
            self.keyboard_layout = Some(keyboard_layout);
        }

        #[diplomat::attr(auto, setter = "keyboard_type")]
        pub fn set_keyboard_type(&mut self, keyboard_type: KeyboardType) {
            self.keyboard_type = Some(keyboard_type.into());
        }

        #[diplomat::attr(auto, setter = "keyboard_subtype")]
        pub fn set_keyboard_subtype(&mut self, keyboard_subtype: u32) {
            self.keyboard_subtype = Some(keyboard_subtype);
        }

        #[diplomat::attr(auto, setter = "keyboard_functional_keys_count")]
        pub fn set_keyboard_functional_keys_count(&mut self, keyboard_functional_keys_count: u32) {
            self.keyboard_functional_keys_count = Some(keyboard_functional_keys_count);
        }

        #[diplomat::attr(auto, setter = "ime_file_name")]
        pub fn set_ime_file_name(&mut self, ime_file_name: &str) {
            self.ime_file_name = Some(ime_file_name.to_owned());
        }

        #[diplomat::attr(auto, setter = "dig_product_id")]
        pub fn set_dig_product_id(&mut self, dig_product_id: &str) {
            self.dig_product_id = Some(dig_product_id.to_owned());
        }

        pub fn set_desktop_size(&mut self, height: u16, width: u16) {
            self.desktop_size = Some(ironrdp::connector::DesktopSize { width, height });
        }

        #[diplomat::attr(auto, setter = "performance_flags")]
        pub fn set_performance_flags(&mut self, performance_flags: &PerformanceFlags) {
            self.performance_flags = Some(performance_flags.0);
        }

        // pub fn set_timezone_info(&mut self, timezone_info: Option<ironrdp::pdu::rdp::client_info::TimezoneInfo>) {
        //     self.timezone_info = timezone_info;
        // }

        #[diplomat::attr(auto, setter = "bitmap_config")]
        pub fn set_bitmap_config(&mut self, bitmap: &BitmapConfig) {
            self.bitmap = Some(bitmap.0.clone());
        }

        #[diplomat::attr(auto, setter = "client_build")]
        pub fn set_client_build(&mut self, client_build: u32) {
            self.client_build = Some(client_build);
        }

        #[diplomat::attr(auto, setter = "client_name")]
        pub fn set_client_name(&mut self, client_name: &str) {
            self.client_name = Some(client_name.to_owned());
        }

        #[diplomat::attr(auto, setter = "client_dir")]
        pub fn set_client_dir(&mut self, client_dir: &str) {
            self.client_dir = Some(client_dir.to_owned());
        }

        #[diplomat::attr(auto, setter = "enable_server_pointer")]
        pub fn set_enable_server_pointer(&mut self, enable_server_pointer: bool) {
            self.enable_server_pointer = Some(enable_server_pointer);
        }

        #[diplomat::attr(auto, setter = "autologon")]
        pub fn set_autologon(&mut self, autologon: bool) {
            self.autologon = Some(autologon);
        }

        #[diplomat::attr(auto, setter = "pointer_software_rendering")]
        pub fn set_pointer_software_rendering(&mut self, pointer_software_rendering: bool) {
            self.pointer_software_rendering = Some(pointer_software_rendering);
        }

        #[diplomat::attr(auto, setter = "dvc_pipe_proxy")]
        pub fn set_dvc_pipe_proxy(&mut self, dvc_pipe_proxy: &DvcPipeProxyConfig) {
            self.dvc_pipe_proxy = Some(dvc_pipe_proxy.clone());
        }

        /// Asks the server to scale its desktop by `scale_factor` percent, 100 to 500, as the local display does.
        ///
        /// It goes in Client Core Data (`desktopScaleFactor`, with the matching `deviceScaleFactor` of 100, 140 or 180,
        /// [MS-RDPBCGR] 2.2.1.3.2) and, for each monitor added with `add_monitor`, in Client Monitor Extended Data
        /// (2.2.1.3.9). Without it, the server keeps its own scale. `build` fails for a factor out of range.
        ///
        /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
        #[diplomat::attr(auto, setter = "desktop_scale_factor")]
        pub fn set_desktop_scale_factor(&mut self, scale_factor: u32) {
            self.desktop_scale_factor = Some(scale_factor);
        }

        /// Adds a monitor to the layout sent in Client Monitor Data ([MS-RDPBCGR] 2.2.1.3.6).
        ///
        /// The bounds are inclusive pixel coordinates, as in `TS_MONITOR_DEF`: a 1920 x 1080 primary monitor is
        /// `(0, 0, 1919, 1079)`.
        /// The primary monitor has its upper-left corner at (0, 0) and the others are placed relative to it, so
        /// their coordinates may be negative.
        ///
        /// With monitors added, `build` checks the layout (1 to 16 monitors, exactly one primary at (0, 0), a union
        /// between 200 x 200 and 32766 x 32766 pixels) and sends the size of the union as the desktop size, in place
        /// of the one given to `set_desktop_size`.
        /// The layout is sent only to a server that supports Extended Client Data Blocks (Windows 7 and Windows
        /// Server 2008 R2 on); another server sees the desktop size alone.
        ///
        /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
        pub fn add_monitor(&mut self, left: i32, top: i32, right: i32, bottom: i32, is_primary: bool) {
            let flags = if is_primary {
                ironrdp::pdu::gcc::MonitorFlags::PRIMARY
            } else {
                ironrdp::pdu::gcc::MonitorFlags::empty()
            };
            self.monitors.push(ironrdp::pdu::gcc::Monitor {
                left,
                top,
                right,
                bottom,
                flags,
            });
        }

        /// Removes the monitors added with `add_monitor`, so `build` sends no Client Monitor Data.
        pub fn clear_monitors(&mut self) {
            self.monitors.clear();
        }

        pub fn build(&self) -> Result<Box<Config>, Box<IronRdpError>> {
            // Checked before the desktop size, so the first missing field is reported first.
            let credentials = self.credentials.clone().ok_or("credentials not set")?;
            let scale = match self.desktop_scale_factor {
                Some(desktop @ 100..=500) => Some((
                    desktop,
                    ironrdp::displaycontrol::pdu::DeviceScaleFactor::from_desktop_scale_factor(desktop).value(),
                )),
                Some(_) => return Err("the desktop scale factor must be 100 to 500 percent".into()),
                None => None,
            };
            let monitor_extended_layout = scale
                .filter(|_| !self.monitors.is_empty())
                .map(|(desktop, device)| super::monitor_extended_layout(&self.monitors, desktop, device));
            let (monitor_layout, desktop_size) = if self.monitors.is_empty() {
                (None, self.desktop_size.ok_or("desktop size not set")?)
            } else {
                let (layout, desktop_size) = super::client_monitor_layout(&self.monitors)?;
                (Some(layout), desktop_size)
            };

            let connector = ironrdp::connector::Config {
                credentials,
                domain: self.domain.clone(),
                enable_tls: self.enable_tls.unwrap_or(false),
                enable_credssp: self.enable_credssp.unwrap_or(true),
                enable_standard_rdp_security: false,
                keyboard_layout: self.keyboard_layout.unwrap_or(0),
                keyboard_type: self
                    .keyboard_type
                    .unwrap_or(ironrdp::pdu::gcc::KeyboardType::IBM_ENHANCED),
                keyboard_subtype: self.keyboard_subtype.unwrap_or(0),
                keyboard_functional_keys_count: self.keyboard_functional_keys_count.unwrap_or(12),
                connection_type: ironrdp::pdu::gcc::ConnectionType::Lan,
                ime_file_name: self.ime_file_name.clone().unwrap_or_default(),
                dig_product_id: self.dig_product_id.clone().unwrap_or_default(),
                desktop_size,
                monitor_layout,
                bitmap: Some(match &self.bitmap {
                    Some(bitmap) => bitmap.clone(),
                    None => super::default_bitmap_config()?,
                }),
                client_build: self.client_build.unwrap_or(0),
                client_name: self.client_name.clone().ok_or("client name not set")?,
                client_dir: self.client_dir.clone().ok_or("client dir not set")?,

                #[cfg(windows)]
                platform: MajorPlatformType::WINDOWS,
                #[cfg(target_os = "macos")]
                platform: MajorPlatformType::MACINTOSH,
                #[cfg(target_os = "ios")]
                platform: MajorPlatformType::IOS,
                #[cfg(target_os = "linux")]
                platform: MajorPlatformType::UNIX,
                #[cfg(target_os = "android")]
                platform: MajorPlatformType::ANDROID,
                #[cfg(target_os = "freebsd")]
                platform: MajorPlatformType::UNIX,
                #[cfg(target_os = "dragonfly")]
                platform: MajorPlatformType::UNIX,
                #[cfg(target_os = "openbsd")]
                platform: MajorPlatformType::UNIX,
                #[cfg(target_os = "netbsd")]
                platform: MajorPlatformType::UNIX,

                enable_server_pointer: self.enable_server_pointer.unwrap_or(false),
                autologon: self.autologon.unwrap_or(false),
                enable_audio_playback: !self.no_audio_playback.unwrap_or(false),
                enable_audio_capture: false,
                request_data: None,
                compression_type: None,
                pointer_software_rendering: self.pointer_software_rendering.unwrap_or(false),
                multitransport_flags: None,
                support_dyn_vc_gfx_protocol: false,
                rdp_version: ironrdp::pdu::gcc::RdpVersion::V5_PLUS,
                performance_flags: self.performance_flags.ok_or("performance flag is missing")?,
                desktop_scale_factor: scale.map_or(0, |(desktop, _)| desktop),
                desktop_physical_size: None,
                desktop_orientation: None,
                device_scale_factor: scale.map(|(_, device)| device),
                monitor_extended_layout,
                relative_mouse_input: false,
                hardware_id: None,
                license_cache: None,
                timezone_info: self.timezone_info.clone().unwrap_or_default(),
                alternate_shell: String::new(),
                work_dir: String::new(),
                remote_application_mode: false,
                rail_support_level: ironrdp::pdu::rdp::capability_sets::RailSupportLevel::empty(),
            };
            let dvc_pipe_proxy = self.dvc_pipe_proxy.clone();

            tracing::debug!(config=?connector, "Built config");

            Ok(Box::new(Config {
                connector,
                dvc_pipe_proxy,
            }))
        }
    }

    #[diplomat::opaque_mut]
    #[derive(Default)]
    pub struct PerformanceFlags(pub ironrdp::pdu::rdp::client_info::PerformanceFlags);

    pub enum PerformanceFlagsType {
        DisableWallpaper,
        DisableFullWindowDrag,
        DisableMenuAnimations,
        DisableTheming,
        Reserved1,
        DisableCursorShadow,
        DisableCursorSettings,
        EnableFontSmoothing,
        EnableDesktopComposition,
        Reserved2,
    }

    impl PerformanceFlags {
        pub fn new_default() -> Box<Self> {
            Box::<PerformanceFlags>::default()
        }

        pub fn new_empty() -> Box<Self> {
            Box::new(PerformanceFlags(
                ironrdp::pdu::rdp::client_info::PerformanceFlags::empty(),
            ))
        }

        pub fn add_flag(&mut self, flag: PerformanceFlagsType) {
            self.0.insert(flag.into());
        }
    }

    #[diplomat::opaque]
    pub struct BitmapConfig(pub ironrdp::connector::BitmapConfig);
}

impl From<PerformanceFlagsType> for PerformanceFlags {
    fn from(val: PerformanceFlagsType) -> Self {
        match val {
            PerformanceFlagsType::DisableCursorSettings => PerformanceFlags::DISABLE_CURSORSETTINGS,
            PerformanceFlagsType::DisableCursorShadow => PerformanceFlags::DISABLE_CURSOR_SHADOW,
            PerformanceFlagsType::DisableFullWindowDrag => PerformanceFlags::DISABLE_FULLWINDOWDRAG,
            PerformanceFlagsType::DisableMenuAnimations => PerformanceFlags::DISABLE_MENUANIMATIONS,
            PerformanceFlagsType::DisableTheming => PerformanceFlags::DISABLE_THEMING,
            PerformanceFlagsType::DisableWallpaper => PerformanceFlags::DISABLE_WALLPAPER,
            PerformanceFlagsType::EnableDesktopComposition => PerformanceFlags::ENABLE_DESKTOP_COMPOSITION,
            PerformanceFlagsType::EnableFontSmoothing => PerformanceFlags::ENABLE_DESKTOP_COMPOSITION,
            PerformanceFlagsType::Reserved1 => PerformanceFlags::RESERVED1,
            PerformanceFlagsType::Reserved2 => PerformanceFlags::RESERVED2,
        }
    }
}

#[cfg(test)]
mod tests {
    use ironrdp::pdu::gcc::{Monitor, MonitorFlags, MonitorOrientation};
    use ironrdp::pdu::rdp::capability_sets::CodecProperty;

    fn complete_builder() -> Box<super::ffi::ConfigBuilder> {
        let mut builder = super::ffi::ConfigBuilder::new();
        builder.with_username_and_password("user", "password");
        builder.set_desktop_size(768, 1024);
        builder.set_client_name("client");
        builder.set_client_dir("C:\\");
        builder.set_performance_flags(&super::ffi::PerformanceFlags::new_default());
        builder
    }

    fn monitor(left: i32, top: i32, right: i32, bottom: i32, primary: bool) -> Monitor {
        Monitor {
            left,
            top,
            right,
            bottom,
            flags: if primary {
                MonitorFlags::PRIMARY
            } else {
                MonitorFlags::empty()
            },
        }
    }

    #[test]
    fn build_sends_the_monitors_and_their_union_as_the_desktop_size() {
        let mut builder = complete_builder();
        builder.add_monitor(0, 0, 1919, 1079, true);
        builder.add_monitor(-1280, 56, -1, 1079, false);

        let Ok(config) = builder.build() else {
            panic!("a valid monitor layout should build");
        };
        let Some(layout) = config.connector.monitor_layout else {
            panic!("the config should carry the monitor layout");
        };

        assert_eq!(
            layout.monitors,
            vec![monitor(0, 0, 1919, 1079, true), monitor(-1280, 56, -1, 1079, false)]
        );
        assert_eq!(config.connector.desktop_size.width, 3200);
        assert_eq!(config.connector.desktop_size.height, 1080);

        let encoded = ironrdp::core::encode_vec(&layout).expect("the layout encodes");
        #[rustfmt::skip]
        let expected: [u8; 48] = [
            0, 0, 0, 0, // flags
            2, 0, 0, 0, // monitorCount
            0, 0, 0, 0, 0, 0, 0, 0, 0x7F, 0x07, 0, 0, 0x37, 0x04, 0, 0, 1, 0, 0, 0, // primary, TS_MONITOR_PRIMARY
            0x00, 0xFB, 0xFF, 0xFF, 0x38, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0x37, 0x04, 0, 0, 0, 0, 0, 0, // left of it
        ];
        assert_eq!(encoded, expected);
    }

    #[test]
    fn build_without_monitors_sends_no_layout() {
        let mut builder = complete_builder();
        builder.add_monitor(0, 0, 1919, 1079, true);
        builder.clear_monitors();

        let Ok(config) = builder.build() else {
            panic!("a builder with every required field should build");
        };

        assert!(config.connector.monitor_layout.is_none());
        assert_eq!(config.connector.desktop_size.width, 1024);
        assert_eq!(config.connector.desktop_size.height, 768);
    }

    #[test]
    fn monitor_layouts_outside_the_specification_are_rejected() {
        let primary = monitor(0, 0, 1919, 1079, true);
        let secondary = monitor(1920, 0, 3839, 1079, false);

        assert!(super::client_monitor_layout(&[]).is_err());
        assert!(super::client_monitor_layout(core::slice::from_ref(&secondary)).is_err());
        assert!(super::client_monitor_layout(&[primary.clone(), primary.clone()]).is_err());
        assert!(super::client_monitor_layout(&[monitor(10, 0, 1919, 1079, true)]).is_err());
        assert!(super::client_monitor_layout(&[monitor(0, 0, -1, 1079, true)]).is_err());
        assert!(super::client_monitor_layout(&[monitor(0, 0, 198, 1079, true)]).is_err());
        assert!(super::client_monitor_layout(&[primary.clone(), monitor(1920, 0, 32_766, 1079, false)]).is_err());

        let mut seventeen = vec![monitor(0, 0, 199, 199, true)];
        seventeen.extend((1..17).map(|index| monitor(index * 200, 0, index * 200 + 199, 199, false)));
        assert!(super::client_monitor_layout(&seventeen).is_err());
        assert!(super::client_monitor_layout(&seventeen[..16]).is_ok());

        assert!(super::client_monitor_layout(&[monitor(0, 0, 199, 199, true)]).is_ok());
        assert!(super::client_monitor_layout(&[primary.clone(), monitor(1920, 0, 32_765, 1079, false)]).is_ok());
        assert!(super::client_monitor_layout(&[primary, secondary]).is_ok());
    }

    #[test]
    fn scale_factor_goes_in_core_data_and_each_monitor() {
        let mut builder = complete_builder();
        builder.set_desktop_scale_factor(150);
        let Ok(config) = builder.build() else {
            panic!("a scale factor in range should build");
        };
        assert_eq!(config.connector.desktop_scale_factor, 150);
        assert_eq!(config.connector.device_scale_factor, Some(140));
        assert!(config.connector.monitor_extended_layout.is_none());

        builder.add_monitor(0, 0, 1919, 1079, true);
        builder.add_monitor(1920, 0, 3119, 1919, false);
        let Ok(config) = builder.build() else {
            panic!("a valid monitor layout should build");
        };
        let Some(extended) = config.connector.monitor_extended_layout else {
            panic!("the config should carry Client Monitor Extended Data");
        };
        let monitors = extended.extended_monitors_info;
        assert_eq!(monitors.len(), 2);
        assert!(
            monitors
                .iter()
                .all(|monitor| monitor.desktop_scale_factor == 150 && monitor.device_scale_factor == 140)
        );
        assert_eq!(monitors[0].orientation, MonitorOrientation::Landscape);
        assert_eq!(monitors[1].orientation, MonitorOrientation::Portrait);
    }

    #[test]
    fn scale_factors_out_of_range_are_rejected() {
        let mut builder = complete_builder();
        builder.set_desktop_scale_factor(99);
        assert!(builder.build().is_err());
        builder.set_desktop_scale_factor(501);
        assert!(builder.build().is_err());
        builder.set_desktop_scale_factor(500);
        assert!(builder.build().is_ok());
    }

    #[test]
    fn build_offers_remotefx_when_no_bitmap_config_is_set() {
        let mut builder = super::ffi::ConfigBuilder::new();
        builder.with_username_and_password("user", "password");
        builder.set_desktop_size(768, 1024);
        builder.set_client_name("client");
        builder.set_client_dir("C:\\");
        builder.set_performance_flags(&super::ffi::PerformanceFlags::new_default());

        let Ok(config) = builder.build() else {
            panic!("a builder with every required field should build");
        };
        let Some(bitmap) = config.connector.bitmap else {
            panic!("the config should carry bitmap settings");
        };

        assert_eq!(bitmap.color_depth, 32);
        assert!(bitmap.lossy_compression);
        assert!(
            bitmap
                .codecs
                .0
                .iter()
                .any(|codec| matches!(codec.property, CodecProperty::RemoteFx(_)))
        );
    }
}
