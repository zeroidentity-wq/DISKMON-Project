# Imbunatatiri propuse pentru DiskMon-Mail

Acest document strange ideile de imbunatatire pentru a face aplicatia mai utila personalului care monitorizeaza serverele RHEL. Directia principala este reducerea zgomotului, cresterea claritatii alertelor si furnizarea de context operational.

## Principii

- Alertele trebuie sa fie actionabile, nu doar informative.
- Email-urile repetitive trebuie reduse prin cooldown si stare persistenta.
- Raportul zilnic trebuie sa ajute la planificare, nu doar sa repete valorile curente.
- Aplicatia trebuie sa ramana simpla, predictibila si usor de rulat pe RHEL 7.9/8.6.
- Integrarea cu sisteme existente de monitorizare trebuie sa fie posibila fara sa inlocuiasca email-ul.

## Prioritate mare

### 1. Histerezis si cooldown pentru alerte

Problema: daca un mount point ramane peste prag, aplicatia poate trimite email la fiecare rulare orara.

Propunere:

- Trimite alerta doar cand statusul trece din OK in ALERTA.
- Retrimite reminder dupa un interval configurabil, de exemplu 6, 12 sau 24 de ore.
- Trimite email de revenire cand ocuparea scade sub un prag de recovery, de exemplu 82% pentru o alerta declansata la 85%.

Exemplu configurare:

```yaml
alert_cooldown_hours: 12
recovery_threshold_percent: 82.0
send_recovery_email: true
```

### 2. Praguri pe severitate

Problema: un singur prag nu diferentiaza intre o situatie de urmarit si una urgenta.

Propunere:

- Warning: 85%
- Critical: 90%
- Emergency: 95%

Email-ul poate pastra forma actuala a subiectului, dar corpul mesajului trebuie sa marcheze clar severitatea.

Exemplu configurare:

```yaml
warning_threshold_percent: 85.0
critical_threshold_percent: 90.0
emergency_threshold_percent: 95.0
```

### 3. Praguri per mount point

Problema: nu toate filesystem-urile au acelasi risc operational.

Propunere:

- Prag implicit global.
- Praguri specifice pentru mount point-uri critice.
- Optional: owner/responsabil pentru fiecare mount point.

Exemplu configurare:

```yaml
default_threshold_percent: 85.0
mounts:
  "/":
    warning: 85.0
    critical: 92.0
    owner: "Linux Ops"
  "/var":
    warning: 80.0
    critical: 90.0
    owner: "App Team"
  "/backup":
    warning: 95.0
    critical: 98.0
    owner: "Backup Team"
```

### 4. Trend si estimare time-to-full

Problema: valoarea curenta arata situatia de acum, dar nu arata viteza cu care se consuma spatiul.

Propunere:

- Salveaza local istoricul ultimelor valori.
- Calculeaza cresterea pe 24h, 7 zile si 30 zile.
- Estimeaza timpul pana la umplere.
- Marcheaza cresterile anormale fata de media recenta.

Exemple in raport:

- `/var a crescut cu 12.4 GB in ultimele 24h`
- `Estimare umplere: 3 zile`
- `Crestere anormala fata de media ultimelor 7 zile`

### 5. Monitorizare inode usage

Problema: un filesystem poate ramane fara inode-uri chiar daca mai are spatiu in GB.

Propunere:

- Colectare `df -i`.
- Afisare inode total, inode utilizate, inode libere si procent ocupare inode.
- Prag separat pentru inode.

Exemplu configurare:

```yaml
inode_threshold_percent: 85.0
```

## Prioritate medie

### 6. Raport zilnic mai util

Raportul zilnic ar trebui sa inceapa cu un sumar executiv:

- Status general.
- Numar mount point-uri OK/WARNING/CRITICAL.
- Top 5 cele mai ocupate mount point-uri.
- Top 5 cele mai mari cresteri fata de ziua precedenta.
- Estimari time-to-full.
- Recomandari de actiune.

### 7. Mesaje de actiune in email

Pentru fiecare alerta, email-ul poate include comenzi utile pentru diagnostic.

Exemple:

```bash
df -h /var
du -xhd1 /var | sort -h
journalctl --disk-usage
```

Pentru directoare cunoscute se pot adauga sugestii specifice, de exemplu `/var/log`, `/tmp`, `/opt`, `/u01`, `/backup`.

### 8. Detectare mount point disparut

Problema: daca un mount point precum `/backup` sau `/data` nu mai este montat, aplicatia poate verifica directorul gol si sa nu observe problema reala.

Propunere:

- Lista de mount point-uri asteptate.
- Alerta separata daca un mount point asteptat lipseste.

Exemplu configurare:

```yaml
expected_mounts:
  - /
  - /var
  - /data
  - /backup
```

### 9. Integrare cu Prometheus/Nagios/Icinga

Propunere:

- Pastrare `--json`.
- Exit codes standard:
  - `0` OK
  - `1` WARNING
  - `2` CRITICAL
  - `3` UNKNOWN
- Output Prometheus textfile pentru Node Exporter textfile collector.

Exemplu metric:

```text
diskmon_filesystem_used_percent{mount="/var"} 87.4
diskmon_filesystem_available_bytes{mount="/var"} 1234567890
```

### 10. Istoric local si comanda status

Propunere:

- Fisier de stare local, de exemplu `/var/lib/diskmon/state.json`.
- Comenzi noi:

```bash
diskmon-mail --status
diskmon-mail --history
```

Starea poate contine:

- ultima rulare;
- ultimul email trimis;
- statusul anterior per mount point;
- valorile recente pentru trend;
- timpul estimat pana la umplere.

## Prioritate mica

### 11. RandomizedDelaySec in systemd timers

Problema: daca aplicatia ruleaza pe multe servere, toate pot trimite email la aceeasi ora.

Propunere pentru timerul orar:

```ini
RandomizedDelaySec=10min
```

Propunere pentru raportul zilnic:

```ini
RandomizedDelaySec=30min
```

### 12. Hardening systemd

Propunere:

```ini
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true
```

Trebuie verificata compatibilitatea exacta cu systemd din RHEL 7.9.

### 13. RPM package

Pentru administrare enterprise, ar fi util un pachet RPM care:

- instaleaza binarul;
- creeaza `/etc/diskmon`;
- instaleaza `config.example.yaml`;
- instaleaza unitatile systemd;
- nu suprascrie config-ul existent la upgrade;
- poate porni/opri timerele prin scripturi post-install.

### 14. Canale alternative de notificare

Email-ul ramane canalul principal, dar pot fi adaugate optional:

- Microsoft Teams webhook;
- Slack webhook;
- generic HTTP webhook;
- SNMP trap pentru NMS clasic.

## Recomandare de implementare

Ordinea recomandata:

1. `state.json` pentru stare persistenta.
2. Histerezis, cooldown si recovery email.
3. Praguri warning/critical/emergency.
4. Praguri per mount point.
5. Monitorizare inode.
6. Trend si estimare time-to-full.
7. Raport zilnic imbunatatit.
8. Exit codes standard si Prometheus textfile output.
9. RPM package si hardening systemd.
