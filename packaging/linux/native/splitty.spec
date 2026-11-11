# Packs a tree that packaging/linux/native/build.sh has staged; it builds
# nothing. The script passes splitty_version and splitty_stage.

Name:           splitty
Version:        %{splitty_version}
Release:        1
Summary:        Multi-session terminal manager
License:        MIT
URL:            https://github.com/npgrosser/splitty

# The files are packed as they were built: no stripping, no debuginfo package.
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none
# xz, which every rpm in use reads; newer rpmbuild would write zstd.
%global _binary_payload w6.xzdio

# By library name, which Fedora and openSUSE share: what the executable links
# (rpm adds these and the glibc versions itself where its file detection
# works) and what it loads at run time: Wayland, EGL and the Vulkan loader.
# The shell integration is sourced by the user's shell, not run.
%global __requires_exclude_from ^/opt/splitty/shell-integration/.*$
Requires:       libxcb.so.1()(64bit)
Requires:       libxkbcommon.so.0()(64bit)
Requires:       libxkbcommon-x11.so.0()(64bit)
Requires:       libwayland-client.so.0()(64bit)
Requires:       libwayland-egl.so.1()(64bit)
Requires:       libEGL.so.1()(64bit)
Requires:       libvulkan.so.1()(64bit)

%description
Splitty is a graphical terminal multiplexer: screens, splits and tabs for
local terminals and for remote hosts over SSH. This package holds the native
desktop app and the splitty command its terminals use.

%install
mkdir -p %{buildroot}
cp -a %{splitty_stage}/. %{buildroot}/

%files
%defattr(-,root,root,-)
/opt/splitty
/usr/bin/splitty-gpui
/usr/share/applications/splitty.desktop
/usr/share/icons/hicolor/512x512/apps/splitty.png
%license /usr/share/licenses/splitty/LICENSE
