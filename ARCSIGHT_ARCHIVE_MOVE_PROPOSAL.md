# Propunere mutare arhive ArcSight pe storage offline

## Decizie actualizata

Conducerea nu a aprobat variantele de mutare prin aplicatie, nici varianta cu storage montat direct si nici varianta prin `rsync`/failover. Directia curenta este read-only:

```text
diskmon-mail-v2 nu muta arhive;
diskmon-mail-v2 nu copiaza arhive;
diskmon-mail-v2 nu sterge arhive.
```

Aplicatia va scana doar path-ul local configurat pentru arhive ArcSight si va afisa in email cele mai mari arhive mai vechi decat un numar de zile configurat. Scopul este sa ofere operatorului o lista clara de directoare candidate pe care le poate muta manual, conform procedurilor aprobate intern.

## Context

Pe serverele SIEM ArcSight, arhivele ocupa rapid spatiul local. ArcSight creeaza directoare de arhiva pe zile, de forma:

```text
19052026
19052026.suppliment
```

Interpretarea operationala propusa:

- directorul `DDMMYYYY` reprezinta arhiva pentru ziua respectiva;
- directorul `DDMMYYYY.suppliment` apare cand arhiva intra in zona offline/suplimentara;
- o pereche completa `DDMMYYYY` + `DDMMYYYY.suppliment` poate fi considerata candidat pentru mutare, dar numai dupa validari suplimentare.

Problema principala este ca serverul pe care ruleaza `diskmon-mail-v2` ajunge sa se umple rapid din cauza arhivelor online si offline. Obiectivul este ca, atunci cand aplicatia detecteaza depasirea pragului de spatiu, sa poata propune sau executa mutarea unor arhive vechi catre storage-ul de arhive offline.

## Arhitectura existenta observata

```text
Server 1 SIEM principal:
55.55.55.1
- aici se umple spatiul cu arhive online/offline

Server 2 SIEM failover:
55.55.55.2
- are mapare catre storage-ul offline

Server 3 ArcSight:
66.66.66.1
- server ArcSight intr-o alta clasa de retea
- arhivele de pe acest server trebuie mutate tot catre storage-ul offline, prin zona/maparea folosita de failover

Storage offline:
110.1.65.100:/ArhiveArcSight
```

Serverul de failover are deja o mapare catre:

```text
110.1.65.100:/ArhiveArcSight
```

Trebuie decis daca storage-ul offline poate fi montat direct si pe serverele ArcSight care produc arhive, in special `55.55.55.1` si `66.66.66.1`. Daca nu, mutarea va trebui facuta prin serverul de failover `55.55.55.2`, care are deja maparea catre storage.

## Varianta recomandata

Recomandarea principala este montarea storage-ului offline direct pe serverul SIEM principal `55.55.55.1`.

Exemplu:

```bash
mount 110.1.65.100:/ArhiveArcSight /mnt/ArhiveArcSight
```

In acest mod, `diskmon-mail-v2` lucreaza cu o cale locala, dar datele ajung pe storage-ul remote:

```text
Sursa locala:
/opt/arcsight/archives/17052026
/opt/arcsight/archives/17052026.suppliment

Destinatie pe storage:
/mnt/ArhiveArcSight/55.55.55.1/17052026
/mnt/ArhiveArcSight/55.55.55.1/17052026.suppliment
```

Este important sa existe subdirectoare separate per server:

```text
/ArhiveArcSight/55.55.55.1/
/ArhiveArcSight/55.55.55.2/
/ArhiveArcSight/66.66.66.1/
```

Astfel se evita suprascrierea sau amestecarea arhivelor intre serverul principal, serverul de failover si al treilea server ArcSight.

## Varianta alternativa

Daca storage-ul offline nu poate fi montat direct pe serverele ArcSight care produc arhive, acestea pot copia prin SSH/rsync catre serverul de failover, iar failover-ul scrie in mount-ul sau catre storage.

Exemplu:

```bash
rsync -aH --partial --delay-updates /opt/arcsight/archives/17052026 \
  archiveuser@55.55.55.2:/mnt/ArhiveArcSight/55.55.55.1/
```

Pentru al treilea server ArcSight:

```bash
rsync -aH --partial --delay-updates /opt/arcsight/archives/17052026 \
  archiveuser@55.55.55.2:/mnt/ArhiveArcSight/66.66.66.1/
```

Aceasta varianta este mai riscanta operational, deoarece trebuie verificat strict ca mount-ul de pe `55.55.55.2` este activ. Daca mount-ul nu este activ, exista risc sa copiem arhivele pe discul local al serverului de failover, nu pe storage-ul offline. Riscul este mai mare daca doua servere ArcSight trimit simultan arhive catre failover.

## Plan operational pentru varianta rsync

Aceasta este varianta in care serverele ArcSight care produc arhive nu monteaza direct storage-ul `110.1.65.100:/ArhiveArcSight`. Ele trimit arhivele prin `rsync` catre serverul de failover `55.55.55.2`, iar serverul de failover scrie in zona sa montata catre storage.

### Topologie

```text
55.55.55.1  --rsync/ssh-->  55.55.55.2  --mount-->  110.1.65.100:/ArhiveArcSight
66.66.66.1  --rsync/ssh-->  55.55.55.2  --mount-->  110.1.65.100:/ArhiveArcSight
```

Destinatii separate pe storage:

```text
/mnt/ArhiveArcSight/55.55.55.1/
/mnt/ArhiveArcSight/66.66.66.1/
```

Serverul `55.55.55.2` trebuie sa fie doar punct de tranzit catre storage, nu destinatie finala pe discul local.

### Cerinte preliminare

Pe serverele sursa `55.55.55.1` si `66.66.66.1`:

```text
- rsync instalat;
- ssh client instalat;
- user dedicat pentru transfer, de exemplu `archiveuser`;
- cheie SSH fara parola pentru rulare automata controlata;
- acces SSH catre `archiveuser@55.55.55.2`;
- drepturi de citire pe directoarele ArcSight care contin arhivele.
```

Pe serverul failover `55.55.55.2`:

```text
- rsync instalat;
- ssh server activ;
- userul `archiveuser` creat si restrictionat catre zona de arhive;
- mount-ul catre `110.1.65.100:/ArhiveArcSight` activ;
- drepturi de scriere in `/mnt/ArhiveArcSight/55.55.55.1/` si `/mnt/ArhiveArcSight/66.66.66.1/`;
- suficient spatiu liber pe storage;
- monitorizare ca mount-ul sa nu cada.
```

### Configuratie propusa pentru rsync

Exemplu pentru `55.55.55.1`:

```yaml
archive_cleanup_enabled: true
archive_move_requires_approval: true

archive_source_dir: /opt/arcsight/archives
archive_transfer_mode: rsync_ssh
archive_rsync_remote_host: 55.55.55.2
archive_rsync_remote_user: archiveuser
archive_rsync_remote_dir: /mnt/ArhiveArcSight/55.55.55.1

archive_min_age_days: 2
archive_trigger_percent: 90.0
archive_stop_percent: 82.0
archive_require_supplement_pair: true
archive_remote_destination_must_be_mount: true
```

Exemplu pentru `66.66.66.1`:

```yaml
archive_cleanup_enabled: true
archive_move_requires_approval: true

archive_source_dir: /opt/arcsight/archives
archive_transfer_mode: rsync_ssh
archive_rsync_remote_host: 55.55.55.2
archive_rsync_remote_user: archiveuser
archive_rsync_remote_dir: /mnt/ArhiveArcSight/66.66.66.1

archive_min_age_days: 2
archive_trigger_percent: 90.0
archive_stop_percent: 82.0
archive_require_supplement_pair: true
archive_remote_destination_must_be_mount: true
```

### Flux de lucru

```text
1. diskmon-mail-v2 detecteaza depasirea pragului pe serverul sursa.
2. Cauta perechi eligibile:
   DDMMYYYY
   DDMMYYYY.suppliment
3. Exclude arhivele curente sau prea recente.
4. Sorteaza candidatii de la cele mai vechi la cele mai noi.
5. Creeaza plan dry-run si trimite email pentru aprobare.
6. Adminul aproba planul prin comanda:
   sudo /usr/bin/diskmon-mail-v2 --approve-archive-move <request-id>
7. Aplicatia verifica prin SSH ca destinatia remote este un mount valid.
8. Aplicatia ruleaza rsync pentru fiecare director din pereche.
9. Aplicatia ruleaza verificari post-copy.
10. Daca verificarile trec, sterge local perechea transferata.
11. Recalculeaza spatiul local.
12. Continua cu urmatoarea pereche pana cand scade sub `archive_stop_percent` sau nu mai exista candidati.
```

### Verificari remote obligatorii

Inainte de orice transfer, aplicatia trebuie sa verifice pe `55.55.55.2`:

```bash
ssh archiveuser@55.55.55.2 'mountpoint /mnt/ArhiveArcSight'
ssh archiveuser@55.55.55.2 'test -w /mnt/ArhiveArcSight/55.55.55.1'
ssh archiveuser@55.55.55.2 'df -P /mnt/ArhiveArcSight'
```

Pentru `66.66.66.1`, verificarea de write trebuie facuta pe directorul lui:

```bash
ssh archiveuser@55.55.55.2 'test -w /mnt/ArhiveArcSight/66.66.66.1'
```

Daca `mountpoint` esueaza, transferul trebuie oprit. Nu trebuie sa existe fallback catre director local pe failover.

### Comenzi rsync propuse

Pentru `55.55.55.1`:

```bash
rsync -aH --numeric-ids --partial --delay-updates --protect-args \
  /opt/arcsight/archives/17052026 \
  archiveuser@55.55.55.2:/mnt/ArhiveArcSight/55.55.55.1/

rsync -aH --numeric-ids --partial --delay-updates --protect-args \
  /opt/arcsight/archives/17052026.suppliment \
  archiveuser@55.55.55.2:/mnt/ArhiveArcSight/55.55.55.1/
```

Pentru `66.66.66.1`:

```bash
rsync -aH --numeric-ids --partial --delay-updates --protect-args \
  /opt/arcsight/archives/17052026 \
  archiveuser@55.55.55.2:/mnt/ArhiveArcSight/66.66.66.1/

rsync -aH --numeric-ids --partial --delay-updates --protect-args \
  /opt/arcsight/archives/17052026.suppliment \
  archiveuser@55.55.55.2:/mnt/ArhiveArcSight/66.66.66.1/
```

Semnificatie optiuni:

- `-a`: mod arhiva, pastreaza structura, permisiuni, timestamp-uri si symlink-uri;
- `-H`: pastreaza hard link-uri;
- `--numeric-ids`: pastreaza UID/GID numeric, fara mapare dupa nume de user;
- `--partial`: pastreaza fisiere partial transferate daca transferul cade;
- `--delay-updates`: face inlocuirile finale la sfarsitul transferului;
- `--protect-args`: protejeaza argumentele trimise prin SSH.

### Verificare dupa transfer

Dupa copierea fiecarei perechi, aplicatia trebuie sa verifice:

```text
- directorul `DDMMYYYY` exista in destinatie;
- directorul `DDMMYYYY.suppliment` exista in destinatie;
- numarul de fisiere sursa/destinatie este identic;
- dimensiunea totala sursa/destinatie este identica;
- un `rsync --dry-run --itemize-changes` nu raporteaza diferente relevante.
```

Exemplu dry-run pentru verificare:

```bash
rsync -aH --numeric-ids --dry-run --itemize-changes --protect-args \
  /opt/arcsight/archives/17052026 \
  archiveuser@55.55.55.2:/mnt/ArhiveArcSight/55.55.55.1/
```

Daca exista diferente, aplicatia nu sterge sursa locala si marcheaza planul ca esuat partial.

### Stergere locala

Stergerea locala trebuie sa fie pas separat si conditionat:

```text
Se sterge local doar daca:
- rsync a terminat cu exit code 0;
- verificarile post-copy au trecut;
- destinatia remote este in continuare mount point;
- perechea nu a fost modificata in timpul transferului;
- planul de mutare este aprobat.
```

Pentru reducerea riscului, se poate introduce o zona de carantina locala:

```text
1. redenumire locala:
   17052026 -> .diskmon-moving-17052026
2. verificare ca ArcSight nu mai foloseste acel director;
3. stergere definitiva dupa transfer si validare.
```

Aceasta carantina trebuie folosita doar daca se confirma ca ArcSight nu depinde de numele original al directorului in acea etapa.

### Control concurenta

Trebuie evitat ca doua procese sa transfere simultan aceeasi arhiva sau sa scrie haotic in aceeasi destinatie.

Recomandari:

```text
- lock local pe fiecare server sursa: `/var/lib/diskmon-v2/archive-move.lock`;
- lock remote pe failover per server sursa:
  `/mnt/ArhiveArcSight/.locks/55.55.55.1.lock`
  `/mnt/ArhiveArcSight/.locks/66.66.66.1.lock`
- subdirectoare separate per server;
- un singur plan activ per server sursa.
```

### Ce trebuie aprobat

Pentru varianta `rsync`, aprobarea ar trebui sa acopere explicit:

```text
- folosirea serverului `55.55.55.2` ca punct de tranzit;
- crearea userului SSH `archiveuser`;
- autentificare cu cheie SSH fara parola;
- restrictii pentru userul `archiveuser`;
- structura directoarelor pe storage;
- stergerea locala dupa verificare;
- numarul minim de zile pastrate local;
- procedura de restore/cautare in arhivele mutate.
```

## Flux propus in diskmon-mail-v2

Aplicatia nu ar trebui sa mute direct arhivele la prima detectie, cel putin nu in faza initiala. Fluxul recomandat este cu aprobare manuala.

```text
1. diskmon-mail-v2 detecteaza ca un mount point a depasit pragul configurat.
2. Aplicatia cauta arhive vechi eligibile pentru mutare.
3. Identifica doar perechi complete:
   DDMMYYYY
   DDMMYYYY.suppliment
4. Genereaza un plan de mutare in mod dry-run.
5. Trimite email cu lista arhivelor propuse pentru mutare.
6. Adminul aproba explicit mutarea printr-o comanda.
7. Aplicatia copiaza arhivele catre storage.
8. Aplicatia verifica destinatia.
9. Aplicatia sterge local doar dupa verificare.
10. Aplicatia se opreste cand spatiul local revine sub pragul de recovery.
```

## Confirmare de la user/admin

Pentru ca `diskmon-mail-v2` ruleaza prin `systemd timer`, nu este potrivit un prompt interactiv de forma:

```text
Vrei sa muti arhivele? [y/N]
```

Serviciul ruleaza in background, fara terminal interactiv. Confirmarea trebuie facuta printr-un mecanism explicit.

### Comenzi propuse

```bash
diskmon-mail-v2 --archive-dry-run
diskmon-mail-v2 --approve-archive-move 20260519-143000
diskmon-mail-v2 --reject-archive-move 20260519-143000
```

La rularea normala prin timer:

```bash
diskmon-mail-v2
```

aplicatia doar detecteaza problema, creeaza cererea de mutare si trimite emailul de confirmare.

Exemplu de mesaj in email:

```text
Spatiu depasit pe /opt/arcsight/archives: 92%

Arhive propuse pentru mutare:
- 16052026 + 16052026.suppliment: 120 GB
- 17052026 + 17052026.suppliment: 98 GB

Pentru aprobare rulati:
sudo /usr/bin/diskmon-mail-v2 --approve-archive-move 20260519-143000
```

Planurile de mutare pot fi salvate in:

```text
/var/lib/diskmon-v2/archive-move-requests.json
```

## Configuratie propusa

Exemplu pentru varianta cu storage montat local pe serverul principal:

```yaml
archive_cleanup_enabled: true
archive_move_requires_approval: true

archive_source_dir: /opt/arcsight/archives
archive_destination_dir: /mnt/ArhiveArcSight/55.55.55.1

archive_min_age_days: 2
archive_trigger_percent: 90.0
archive_stop_percent: 82.0
archive_require_supplement_pair: true
archive_destination_must_be_mount: true
```

Semnificatie:

- `archive_cleanup_enabled`: activeaza logica de cautare/mutare arhive;
- `archive_move_requires_approval`: cere aprobare manuala inainte de mutare;
- `archive_source_dir`: directorul local unde ArcSight creeaza arhivele;
- `archive_destination_dir`: directorul de pe storage-ul offline;
- `archive_min_age_days`: nu muta arhive mai noi de acest numar de zile;
- `archive_trigger_percent`: pragul de ocupare de la care se genereaza planul;
- `archive_stop_percent`: aplicatia se opreste din mutari cand ocuparea scade sub acest prag;
- `archive_require_supplement_pair`: muta doar perechi complete `DDMMYYYY` + `DDMMYYYY.suppliment`;
- `archive_destination_must_be_mount`: refuza mutarea daca destinatia nu este mount point.

## Reguli de siguranta

Mutarea trebuie facuta conservator:

- nu se muta arhiva pentru ziua curenta;
- nu se muta arhive mai noi decat `archive_min_age_days`;
- nu se muta directoare fara perechea `.suppliment`, daca `archive_require_supplement_pair` este activ;
- nu se muta daca destinatia nu este montata;
- nu se muta daca destinatia este read-only;
- nu se muta daca destinatia nu are spatiu suficient;
- nu se suprascriu directoare deja existente in destinatie;
- nu se sterge nimic local pana cand copierea nu este verificata;
- se foloseste lock file ca sa nu ruleze doua mutari in paralel;
- se logheaza fiecare actiune in jurnalul systemd si optional intr-un fisier dedicat.

Verificari utile inainte de mutare:

```bash
mountpoint /mnt/ArhiveArcSight
df -h /mnt/ArhiveArcSight
touch /mnt/ArhiveArcSight/.diskmon-write-test
```

## Copiere si verificare

Mutarea ar trebui facuta in doua faze: copiere, apoi stergere.

Copiere:

```bash
rsync -aH --partial --delay-updates /opt/arcsight/archives/17052026 \
  /mnt/ArhiveArcSight/55.55.55.1/

rsync -aH --partial --delay-updates /opt/arcsight/archives/17052026.suppliment \
  /mnt/ArhiveArcSight/55.55.55.1/
```

Dupa copiere, aplicatia ar trebui sa verifice:

```text
- directorul exista in destinatie;
- numarul de fisiere este identic;
- dimensiunea totala este identica sau compatibila;
- un dry-run rsync nu mai raporteaza diferente semnificative.
```

Stergerea locala se face doar dupa aceste verificari.

## Riscuri de clarificat inainte de implementare

Trebuie obtinuta aprobare si clarificare pentru urmatoarele puncte:

- ArcSight mai are nevoie sa acceseze arhivele offline din calea originala?
- Exista o procedura ArcSight oficiala pentru mutarea/restaurarea arhivelor offline?
- Storage-ul `110.1.65.100:/ArhiveArcSight` poate fi montat direct pe `55.55.55.1`?
- Storage-ul `110.1.65.100:/ArhiveArcSight` poate fi montat direct si pe `66.66.66.1`?
- Ce protocol se foloseste pentru mapare: NFS, CIFS/SMB, altceva?
- Ce user va avea drepturi de scriere pe storage?
- Care este path-ul real al arhivelor ArcSight pe serverul principal?
- Care este path-ul real al arhivelor ArcSight pe serverul `66.66.66.1`?
- Cate zile trebuie pastrate local inainte de mutare?
- Este acceptata stergerea locala automata dupa verificare sau se doreste doar copiere cu stergere manuala?
- Se doreste ca failover-ul sa poata vedea/restaura arhivele mutate?

## Recomandare pentru faza initiala

Pentru prima implementare, recomandarea este:

```text
1. Activare doar dry-run si email cu propuneri.
2. Aprobare manuala obligatorie pentru fiecare plan.
3. Copiere cu rsync catre storage.
4. Verificare post-copy.
5. Stergere locala doar dupa aprobare si verificare.
6. Trecere la auto-move doar dupa cateva cicluri validate operational.
```

Dupa validare, se poate permite mutarea automata pentru arhive mai vechi de un prag mai conservator, de exemplu:

```yaml
archive_move_requires_approval: false
archive_min_age_days: 7
```

Aceasta schimbare ar trebui facuta numai dupa confirmarea echipei SIEM/ArcSight si dupa validarea procesului de restore/cautare din arhivele mutate.
