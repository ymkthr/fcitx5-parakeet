%global sherpa_version 1.13.7
%global sherpa_archive sherpa-onnx-v%{sherpa_version}-linux-x64-shared-no-tts

Name:           fcitx5-voice-ja
Version:        0.4.0
Release:        1%{?dist}
Summary:        Offline Japanese and English speech input for fcitx5 using NVIDIA Parakeet
# MIT: this project, ONNX Runtime and llama.cpp; Apache-2.0: the bundled
# sherpa-onnx libraries; IPADIC: the dictionary embedded in voice-jad.
License:        MIT AND Apache-2.0 AND LicenseRef-IPADIC
URL:            https://github.com/ymkthr/fcitx5-voice-ja
Source0:        %{name}-%{version}.tar.gz
Source1:        https://github.com/k2-fsa/sherpa-onnx/releases/download/v%{sherpa_version}/%{sherpa_archive}.tar.bz2
ExclusiveArch:  x86_64

BuildRequires:  cargo >= 1.88
BuildRequires:  rust >= 1.88
BuildRequires:  clang-devel
BuildRequires:  cmake
BuildRequires:  gcc-c++
BuildRequires:  extra-cmake-modules
BuildRequires:  gettext
BuildRequires:  fcitx5-devel
BuildRequires:  pipewire-devel
BuildRequires:  systemd-rpm-macros
Requires:       fcitx5
Requires:       pipewire
Recommends:     curl
Obsoletes:      fcitx5-parakeet < 0.4.0
Provides:       fcitx5-parakeet = %{version}-%{release}
# The bundled sherpa-onnx / onnxruntime libraries are private to voice-jad and
# ship without build-ids, so debuginfo extraction is skipped for the package.
%global __requires_exclude ^lib(sherpa-onnx-c-api|onnxruntime)\\.so.*$
%global __provides_exclude_from ^%{_libdir}/voice-jad/.*$
%global debug_package %{nil}

%description
fcitx5-voice-ja adds push-to-talk dictation to fcitx5 using NVIDIA Parakeet
models running locally through sherpa-onnx. It ships an fcitx5 addon and a
per-user daemon (voice-jad) that captures audio from PipeWire.

The speech models are not included; run voice-jad-download-models as your
desktop user after installing.

%prep
%setup -q -n %{name}-%{version} -a 1

%build
cmake -S fcitx5 -B fcitx-build \
  -DCMAKE_BUILD_TYPE=None \
  -DCMAKE_INSTALL_PREFIX=%{_prefix} \
  -DBUILD_TESTING=OFF
cmake --build fcitx-build

export SHERPA_ONNX_LIB_DIR="$PWD/%{sherpa_archive}/lib"
export SHERPA_ONNX_RPATH=%{_libdir}/voice-jad
# ggml turns every SIMD option off when SOURCE_DATE_EPOCH is set, which
# rpmbuild does; voice-jad checks for these at start-up.
export GGML_SSE42=ON GGML_AVX=ON GGML_AVX2=ON GGML_BMI2=ON GGML_FMA=ON GGML_F16C=ON
cargo build --manifest-path daemon/Cargo.toml --release --locked

%install
packaging/stage.sh %{buildroot} "$PWD/%{sherpa_archive}/lib" %{_libdir} %{_licensedir}/%{name}
%find_lang %{name}

%files -f %{name}.lang
%license %{_licensedir}/%{name}/LICENSE
%license %{_licensedir}/%{name}/LICENSE-APACHE-2.0
%license %{_licensedir}/%{name}/onnxruntime.LICENSE
%license %{_licensedir}/%{name}/llama.cpp.LICENSE
%license %{_licensedir}/%{name}/ipadic.LICENSE
%license %{_licensedir}/%{name}/onnxruntime-ThirdPartyNotices.txt
%license %{_licensedir}/%{name}/rust-crates.LICENSE
%doc %{_docdir}/%{name}/THIRD_PARTY_NOTICES.md
%doc %{_docdir}/%{name}/config.example.toml
%{_bindir}/voice-jad
%{_bindir}/voice-ja-ctl
%{_bindir}/voice-jad-download-models
%{_bindir}/voice-ja-capslock-menu
%{_libdir}/fcitx5/voiceja.so
%{_libdir}/voice-jad/
%{_datadir}/fcitx5/addon/voiceja.conf
%{_userunitdir}/voice-jad.service
%{_userunitdir}/voice-jad.socket

%changelog
* Sun Oct 04 2026 ymkthr <ymkthr@users.noreply.github.com> - 0.4.0-1
- Rename fcitx5-parakeet to fcitx5-voice-ja.

* Sun Sep 13 2026 ymkthr <ymkthr@users.noreply.github.com> - 0.3.0-1
- Initial RPM packaging.
