macro_rules! platforms {
    ($($($platform:expr);* => $module:ident),*) => {
        $(
            #[cfg(any($(target_os = $platform),*))]
            #[cfg_attr(not(any($(target_os = $platform),*)), allow(dead_code))]
            mod $module;

            #[cfg(any($(target_os = $platform),*))]
            pub use self::$module::*;

            // muxterm 本地补丁：Linux / Android 没有 `ENOATTR`，等价错误码是
            // `ENODATA`。上游 0.2.3 在 `cfg(target_os = "linux")` 下直接引用
            // `libc::ENOATTR`，导致整个 crate 在 Linux 上编译失败（gpui 依赖链
            // gpui_http_client → zed-async-tar → xattr 会被拖死）。
            #[cfg(all(
                any($(target_os = $platform),*),
                any(target_os = "linux", target_os = "android")
            ))]
            pub const ENOATTR: ::libc::c_int = ::libc::ENODATA;
            #[cfg(all(
                any($(target_os = $platform),*),
                not(any(target_os = "linux", target_os = "android"))
            ))]
            pub const ENOATTR: ::libc::c_int = ::libc::ENOATTR;
        )*

        #[cfg(all(feature = "unsupported", not(any($($(target_os = $platform),*),*))))]
        #[cfg_attr(any($($(target_os = $platform),*),*), allow(dead_code))]
        mod unsupported;

        #[cfg(all(feature = "unsupported", not(any($($(target_os = $platform),*),*))))]
        pub use self::unsupported::*;
        #[cfg(all(feature = "unsupported", not(any($($(target_os = $platform),*),*))))]
        pub const ENOATTR: ::libc::c_int = 0;


        /// A constant indicating whether or not the target platform is supported.
        ///
        /// To make programmer's lives easier, this library builds on all platforms.
        /// However, all function calls on unsupported platforms will return
        /// `io::Error`s.
        ///
        /// Note: If you would like compilation to simply fail on unsupported platforms,
        /// turn of the `unsupported` feature.
        pub const SUPPORTED_PLATFORM: bool = cfg!(any($($(target_os = $platform),*),*));
    }
}

platforms! {
    "android"; "linux"; "macos" => linux_macos,
    "freebsd"; "netbsd" => bsd
}
