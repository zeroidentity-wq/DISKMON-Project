Name:           diskmon-mail-v2
Version:        0.3.0
Release:        1%{?dist}
Summary:        Disk space monitoring with email reports and trend analysis
License:        Proprietary
URL:            https://example.invalid/diskmon-mail-v2

Source0:        diskmon-mail-v2
Source1:        config.example.yaml
Source2:        diskmon-v2.service
Source3:        diskmon-v2.timer
Source4:        diskmon-v2-force.service
Source5:        diskmon-v2-force.timer

Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd
BuildRequires:  systemd-rpm-macros

%description
DiskMon Mail v2 monitors local mount points, sends email alerts, keeps alert
state, calculates storage trend, and sends daily reports through systemd timers.

%prep

%build

%install
install -D -m 0755 %{SOURCE0} %{buildroot}%{_bindir}/diskmon-mail-v2
install -d -m 0755 %{buildroot}%{_sysconfdir}/diskmon-v2
install -m 0644 %{SOURCE1} %{buildroot}%{_sysconfdir}/diskmon-v2/config.example.yaml
install -m 0600 %{SOURCE1} %{buildroot}%{_sysconfdir}/diskmon-v2/config.yaml
install -d -m 0755 %{buildroot}%{_sharedstatedir}/diskmon-v2
install -D -m 0644 %{SOURCE2} %{buildroot}%{_unitdir}/diskmon-v2.service
install -D -m 0644 %{SOURCE3} %{buildroot}%{_unitdir}/diskmon-v2.timer
install -D -m 0644 %{SOURCE4} %{buildroot}%{_unitdir}/diskmon-v2-force.service
install -D -m 0644 %{SOURCE5} %{buildroot}%{_unitdir}/diskmon-v2-force.timer

%post
%systemd_post diskmon-v2.service diskmon-v2.timer diskmon-v2-force.service diskmon-v2-force.timer
systemctl enable --now diskmon-v2.timer >/dev/null 2>&1 || true
systemctl enable --now diskmon-v2-force.timer >/dev/null 2>&1 || true

%preun
%systemd_preun diskmon-v2.service diskmon-v2.timer diskmon-v2-force.service diskmon-v2-force.timer

%postun
%systemd_postun_with_restart diskmon-v2.service diskmon-v2-force.service

%files
%{_bindir}/diskmon-mail-v2
%dir %{_sysconfdir}/diskmon-v2
%config(noreplace) %{_sysconfdir}/diskmon-v2/config.yaml
%{_sysconfdir}/diskmon-v2/config.example.yaml
%dir %{_sharedstatedir}/diskmon-v2
%{_unitdir}/diskmon-v2.service
%{_unitdir}/diskmon-v2.timer
%{_unitdir}/diskmon-v2-force.service
%{_unitdir}/diskmon-v2-force.timer

%changelog
* Tue May 19 2026 DiskMon <root@localhost> - 0.3.0-1
- Initial v2 RPM package with separate binary, config, state and systemd timers.
