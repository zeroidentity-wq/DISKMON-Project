# Build si Deploy DiskMon v2

## 1. Build RPM

Din radacina proiectului:

```bash
packaging/rpm/build-rpm-v2.sh
```

RPM-ul rezultat:

```text
target/rpmbuild/RPMS/x86_64/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm
```

## 2. Copiere pe server

```bash
scp target/rpmbuild/RPMS/x86_64/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm user@server:/tmp/
```

## 3. Install / Upgrade pe server

Pentru instalare initiala:

```bash
sudo dnf install /tmp/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm
```

Daca pachetul este deja instalat, foloseste upgrade:

```bash
sudo dnf upgrade /tmp/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm
```

Pe RHEL 7, daca nu ai `dnf`:

```bash
sudo yum install /tmp/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm
```

Comanda completa recomandata, merge si pentru install si pentru upgrade:

```bash
sudo dnf -y install /tmp/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm || \
sudo dnf -y upgrade /tmp/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm
```

## 4. Configurare

Editeaza:

```bash
sudo vi /etc/diskmon-v2/config.yaml
```

Verifica minim:

```yaml
mail_enabled: true
smtp_server: smtp.example.com
smtp_port: 587
smtp_user: user@example.com
smtp_pass: password
email_from: admin@example.com
email_to: alerts@example.com
smtp_security: starttls
warning_threshold_percent: 85.0
critical_threshold_percent: 90.0
emergency_threshold_percent: 95.0
alert_cooldown_hours: 12
recovery_threshold_percent: 82.0
send_recovery_email: true
friendly_name: "Server productie"
```

Permisiuni:

```bash
sudo chown root:root /etc/diskmon-v2/config.yaml
sudo chmod 600 /etc/diskmon-v2/config.yaml
sudo mkdir -p /var/lib/diskmon-v2
sudo chmod 755 /var/lib/diskmon-v2
```

## 5. Activare servicii si timere

Ruleaza dupa install/upgrade:

```bash
sudo systemctl daemon-reload
sudo systemctl enable diskmon-v2.timer
sudo systemctl enable diskmon-v2-force.timer
sudo systemctl start diskmon-v2.timer
sudo systemctl start diskmon-v2-force.timer
```

Varianta scurta:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now diskmon-v2.timer
sudo systemctl enable --now diskmon-v2-force.timer
```

Serviciile sunt `oneshot`, deci nu trebuie activate ca servicii permanente. Ele se ruleaza manual sau prin timer:

```bash
sudo systemctl start diskmon-v2.service
sudo systemctl start diskmon-v2-force.service
```

## 6. Test rapid

Raport complet, trimite email indiferent de prag:

```bash
sudo systemctl start diskmon-v2-force.service
sudo journalctl -u diskmon-v2-force.service -n 100 --no-pager
```

Verificare normala:

```bash
sudo systemctl start diskmon-v2.service
sudo journalctl -u diskmon-v2.service -n 100 --no-pager
```

Rulare directa:

```bash
sudo DISKMON_CONFIG_PATH=/etc/diskmon-v2/config.yaml \
     DISKMON_STATE_PATH=/var/lib/diskmon-v2/diskmon-state.json \
     /usr/bin/diskmon-mail-v2 --force-mail
```

## 7. Verificari systemd

Verifica:

```bash
sudo systemctl daemon-reload
systemctl status diskmon-v2.timer
systemctl status diskmon-v2-force.timer
systemctl list-timers 'diskmon-v2*'
```

Verifica unitatile instalate:

```bash
systemctl cat diskmon-v2.service
systemctl cat diskmon-v2.timer
systemctl cat diskmon-v2-force.service
systemctl cat diskmon-v2-force.timer
```

Verifica logurile:

```bash
sudo journalctl -u diskmon-v2.service -n 100 --no-pager
sudo journalctl -u diskmon-v2-force.service -n 100 --no-pager
sudo journalctl -u diskmon-v2.service -u diskmon-v2-force.service --since "1 hour ago" --no-pager
```

Verifica state file:

```bash
sudo ls -l /var/lib/diskmon-v2/
sudo cat /var/lib/diskmon-v2/diskmon-state.json
```

## 8. Comenzi complete pentru un server nou

Inlocuieste calea RPM daca este diferita:

```bash
sudo dnf -y install /tmp/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm

sudo chown root:root /etc/diskmon-v2/config.yaml
sudo chmod 600 /etc/diskmon-v2/config.yaml
sudo mkdir -p /var/lib/diskmon-v2
sudo chmod 755 /var/lib/diskmon-v2

sudo systemctl daemon-reload
sudo systemctl enable --now diskmon-v2.timer
sudo systemctl enable --now diskmon-v2-force.timer

sudo systemctl start diskmon-v2-force.service
sudo journalctl -u diskmon-v2-force.service -n 100 --no-pager

systemctl list-timers 'diskmon-v2*'
```

## 9. Comenzi complete pentru upgrade

```bash
sudo dnf -y upgrade /tmp/diskmon-mail-v2-0.3.0-1.el10.x86_64.rpm

sudo systemctl daemon-reload
sudo systemctl restart diskmon-v2.timer
sudo systemctl restart diskmon-v2-force.timer

sudo systemctl start diskmon-v2-force.service
sudo journalctl -u diskmon-v2-force.service -n 100 --no-pager

systemctl list-timers 'diskmon-v2*'
```

## 10. Oprire / dezactivare v2

```bash
sudo systemctl stop diskmon-v2.timer
sudo systemctl stop diskmon-v2-force.timer
sudo systemctl disable diskmon-v2.timer
sudo systemctl disable diskmon-v2-force.timer
```

## 11. Fisiere instalate

```text
/usr/bin/diskmon-mail-v2
/etc/diskmon-v2/config.yaml
/etc/diskmon-v2/config.example.yaml
/var/lib/diskmon-v2/diskmon-state.json
/usr/lib/systemd/system/diskmon-v2.service
/usr/lib/systemd/system/diskmon-v2.timer
/usr/lib/systemd/system/diskmon-v2-force.service
/usr/lib/systemd/system/diskmon-v2-force.timer
```

Pachetul v2 nu foloseste unitatile vechi `diskmon.service` / `diskmon.timer`, deci poate rula in paralel cu instalarea existenta.
