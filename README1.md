# DiskMon-Mail

Monitor de spatiu de stocare pentru servere Linux/RHEL. Programul citeste mount point-urile locale si trimite email cand gradul de ocupare depaseste pragul configurat.

## Comportament

- Pragul `threshold_percent` inseamna procent ocupat, nu procent liber.
- Exemplu: `threshold_percent: 85.0` trimite alarma cand un mount point ajunge la 85% ocupat sau mai mult.
- Email-ul contine informatii de stocare, severitate, trend de crestere si estimare pana la umplere.
- Nu mai sunt folosite verificari de stare fizica a discurilor; raportul este strict despre spatiu.
- `diskmon-mail-v2` ruleaza o verificare normala si trimite email doar daca exista depasire de prag.
- `diskmon-mail-v2 --force-mail` trimite raport complet indiferent de prag. Acesta este modul pentru raport zilnic sau raport la cerere.
- `diskmon-mail-v2 --json` afiseaza rezultatul in format JSON pentru integrare cu alte sisteme.

## Configurare

Fisierul `config.yaml` trebuie sa fie in directorul de lucru al programului sau indicat prin `DISKMON_CONFIG_PATH`. Pentru instalarea v2 prin RPM, calea este `/etc/diskmon-v2/config.yaml`.

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
excluded_disks: [""]
friendly_name: "Server productie"
debug: false
```

Optiuni importante:

- `warning_threshold_percent`: pragul de intrare in alerta, severitate WARNING.
- `critical_threshold_percent`: pragul pentru severitate CRITICAL.
- `emergency_threshold_percent`: pragul pentru severitate EMERGENCY.
- `threshold_percent`: prag vechi, inca acceptat ca fallback pentru `warning_threshold_percent`.
- `alert_cooldown_hours`: intervalul minim dupa care se retrimite reminder daca alerta ramane activa.
- `recovery_threshold_percent`: pragul sub care o alerta este considerata revenita. Trebuie sa fie mai mic decat pragul WARNING.
- `send_recovery_email`: trimite email de revenire cand ocuparea scade sub `recovery_threshold_percent`.
- `excluded_disks`: lista de mount point-uri sau device-uri excluse.
- `friendly_name`: nume prietenos folosit in subiectul si corpul email-ului.
- `email_to`: accepta mai multi destinatari separati prin virgula.
- `smtp_security`: `none`, `starttls` sau `ssl`.

Starea alertelor este persistata in `diskmon-state.json` in directorul de lucru al programului sau in calea indicata prin `DISKMON_STATE_PATH`. Cu unitatile systemd v2, fisierul va fi in `/var/lib/diskmon-v2/diskmon-state.json`.
In acelasi fisier este pastrat istoricul ultimelor valori pentru trend, cu retentie locala de aproximativ 31 de zile.

Reguli de trimitere pentru rularea periodica:

- trimite alerta cand un mount point trece din OK in ALERTA;
- trimite notificare imediata daca un mount point activ escaladeaza la o severitate mai mare;
- nu retrimite cat timp alerta ramane activa, pana expira `alert_cooldown_hours`;
- trimite reminder dupa expirarea cooldown-ului;
- trimite email de revenire cand ocuparea scade la `recovery_threshold_percent` sau mai jos, daca `send_recovery_email` este `true`.

Trendul din raport include:

- cresterea spatiului utilizat in ultimele 24h, 7 zile si 30 zile;
- estimarea timpului pana la umplere, calculata din rata recenta de crestere;
- marcaj pentru crestere anormala cand ultimele 24h depasesc semnificativ media ultimelor 7 zile.

Raportul zilnic trimis cu `--force-mail` incepe cu un sumar executiv:

- status general;
- numar de mount point-uri OK, WARNING, CRITICAL si EMERGENCY;
- top 5 cele mai ocupate mount point-uri;
- top 5 cele mai mari cresteri fata de ziua precedenta;
- cele mai apropiate estimari time-to-full;
- recomandari de actiune.

Credentialele SMTP pot fi suprascrise prin variabile de mediu:

```bash
export DISKMON_SMTP_USER="user@example.com"
export DISKMON_SMTP_PASS="parola"
export DISKMON_EMAIL_FROM="monitoring@example.com"
export DISKMON_EMAIL_TO="admin1@example.com,admin2@example.com"
```

## Rulare

```bash
# Verificare normala: trimite email doar daca gradul de ocupare depaseste pragul
./diskmon-mail-v2

# Raport complet la cerere
./diskmon-mail-v2 --force-mail

# Output JSON
./diskmon-mail-v2 --json
```

Forma subiectului foloseste marcaje Unicode pentru citire rapida in inbox:

- `🔴 [ALARMĂ] [DEPĂSIRE PRAG DE STOCARE] System Disk Report - <nume> (<sistem>)`
- `🔴 [REMINDER ALARMĂ] [STOCARE] System Disk Report - <nume> (<sistem>)`
- `🔴 [RAPORT ALERTĂ] [RAPORT ZILNIC] System Disk Report - <nume> (<sistem>)`
- `🟢 [RAPORT OK] [RAPORT ZILNIC] System Disk Report - <nume> (<sistem>)`
- `🟢 [REVENIRE OK] [STOCARE] System Disk Report - <nume> (<sistem>)`

## Systemd

Unitatile v2 din `packaging/systemd` folosesc doua timere si pot rula in paralel cu unitatile vechi `diskmon.timer` / `diskmon-force.timer`:

- `diskmon-v2.timer`: ruleaza verificarea normala din ora in ora.
- `diskmon-v2-force.timer`: ruleaza raportul complet zilnic la 07:00.

Instalare exemplu:

```bash
sudo mkdir -p /etc/diskmon-v2 /var/lib/diskmon-v2
sudo cp target/x86_64-unknown-linux-musl/release/diskmon-mail-v2 /usr/bin/diskmon-mail-v2
sudo cp src/linux/config.example.yaml /etc/diskmon-v2/config.yaml
sudo cp packaging/systemd/diskmon-v2.service /etc/systemd/system/
sudo cp packaging/systemd/diskmon-v2.timer /etc/systemd/system/
sudo cp packaging/systemd/diskmon-v2-force.service /etc/systemd/system/
sudo cp packaging/systemd/diskmon-v2-force.timer /etc/systemd/system/
sudo chown root:root /etc/diskmon-v2/config.yaml
sudo chmod 600 /etc/diskmon-v2/config.yaml
sudo systemctl daemon-reload
sudo systemctl enable --now diskmon-v2.timer
sudo systemctl enable --now diskmon-v2-force.timer
```

Rulari manuale prin systemd:

```bash
# Check normal acum
sudo systemctl start diskmon-v2.service

# Raport complet acum
sudo systemctl start diskmon-v2-force.service
```

## RPM v2

Pachetul RPM v2 este definit in `packaging/rpm/diskmon-mail-v2.spec`. El instaleaza:

- `/usr/bin/diskmon-mail-v2`;
- `/etc/diskmon-v2/config.yaml` ca `%config(noreplace)`, deci nu suprascrie configuratia existenta la upgrade;
- `/etc/diskmon-v2/config.example.yaml`;
- `/var/lib/diskmon-v2`;
- unitatile systemd `diskmon-v2*`.

Build RPM local:

```bash
packaging/rpm/build-rpm-v2.sh
```

RPM-ul rezultat va fi in:

```text
target/rpmbuild/RPMS/
```

## Build musl pentru RHEL 7.9/8.6

Proiectul are `Cross.toml` pentru target musl.

```bash
cross build --release --target x86_64-unknown-linux-musl
```

Alternativ, daca toolchain-ul musl este instalat local:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Binarul rezultat:

```text
target/x86_64-unknown-linux-musl/release/diskmon-mail-v2
```
