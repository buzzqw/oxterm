# Gestore connessioni SSH: stato del lavoro

Documento di passaggio per chi prosegue, persona o agente AI. Aggiornalo a ogni
sessione: in fondo c'è un registro. Ultimo aggiornamento: 2026-11-01.

- **Branch:** `feat/ssh-connections`, partito da `master` a `fd0cc01`.
- **Worktree di sviluppo:** `/home/andres/oxterm-connections`. Si può eliminare
  dopo il merge con `git worktree remove`.
- **Proposta iniziale:** <https://claude.ai/artifact/LQG2ZPbuyUYrmMkxrPTZE4>
- **Regole del repository:** `AGENTS.md` e `CONTRIBUTING.md`. In particolare, la
  logica senza display sta in `src/headless/` e dopo aver cambiato
  `src/headless/spec.rs` va eseguito `cargo run --bin oxterm-docgen`.

## Decisioni di progetto

Non vanno riaperte senza un motivo nuovo.

1. **OpenSSH è l'unico motore.** Oxterm esegue `ssh` di sistema. Niente client
   interno come `russh`.
2. **`~/.ssh/config` si legge e non si scrive mai.** Un test lo verifica.
3. **Nessun segreto nei file di Oxterm.** Le password stanno nel Secret Service,
   tramite `secret-tool`. Niente crittografia scritta da noi.
4. **Formato pubblico.** `connections.json` sarà letto anche dalla futura TUI. I
   campi sconosciuti sopravvivono alla riscrittura (`#[serde(flatten)] unknown`).
5. **File danneggiato o più recente: mai sovrascritto.** Il primo dà errore, il
   secondo si apre in sola lettura (`version > CURRENT_VERSION`).
6. **L'helper delle password usa una lista di consenso.** Risponde solo a
   `user@host's password:` o `(user@host) Password:` della destinazione, una sola
   volta per processo `ssh`. Tutto il resto va a `/dev/tty`. Non aggiungere prompt
   generici: un `Password:` può arrivare da un bastion.
7. **Le connessioni non ripartono da sole.** Dopo un riavvio Oxterm chiede conferma.
8. **La TUI è fuori da questo lavoro.** Quando partirà, il nucleo si estrae in un
   crate e i pannelli incorporati useranno `libghostty-vt`, preferenza espressa
   dall'autore. Oxterm non dipende da PCM, né PCM da Oxterm.

## Architettura

| File | Ruolo | GTK |
| --- | --- | --- |
| `src/headless/connections.rs` | Rubrica, validazione, argomenti `ssh`, unione con `ssh_config`, albero, ricerca, import | No |
| `src/headless/tunnels.rs` | Processi `ssh -N` in background e il loro stato reale | No |
| `src/headless/askpass.rs` | Decisione sui prompt, tentativi, `secret-tool` | No |
| `src/headless/persistence.rs` | Spostato da `src/` per renderlo riutilizzabile | No |
| `src/headless/ssh_config.rs` | Aggiunto `effective_config`, un `ssh -G` con timeout | No |
| `src/bin/oxterm-askpass.rs` | Helper `SSH_ASKPASS` | No |
| `src/headless/control_cli.rs` | Parsing di `connect`, `connections`, `tunnel` | No |
| `src/control.rs` | Lato client dei comandi e operazione `tunnel` | Sì |
| `src/connections_panel.rs` | Widget del pannello | Sì |
| `src/connections_form.rs` | Dialogo di modifica | Sì |
| `src/connections_ui.rs` | Comportamento su `MainWindow`: connessione, menu, tunnel | Sì |

`src/window.rs` contiene solo lo slot del pannello e pochi collegamenti: il resto
va tenuto fuori da lì, perché il file supera già le 6.500 righe.

Flusso di apertura:

1. `connect_id` o `connect_text` trovano la voce.
2. Un thread esegue `connections::prepare_session`, con `ssh -G` limitato a 2 s.
3. `finish_connect`, sul thread GUI, apre il terminale con `add_new_tab`, associa
   id e nome con `set_connection` e fissa il titolo con `pin_tab_title`.
4. Registra l'uso in un thread e avvia i tunnel con `autostart`.

La CLI calcola il comando nel client, perché le richieste di controllo sono servite
sul thread GUI, che non deve aspettare `ssh -G`.

## Stato delle fasi

| Fase | Contenuto | Stato |
| --- | --- | --- |
| 0 | `ssh -G` fuori dal thread GUI | Fatto |
| 0 | Verifica dell'integrazione remota | Fatto: limite documentato nel manuale |
| 1 | Nucleo, `connections.json`, pannello, ricerca, connessione | Fatto |
| 2 | Form, anteprima comando, CLI, import ed export | Fatto |
| 3 | Tunnel salvati con stato reale | Fatto |
| 4 | Helper askpass e Secret Service | Fatto, tranne la prova con password reale |
| 5 | Riapertura dei tab SSH con conferma | Fatto |
| 5 | Colori per singola connessione | **Da fare** |

## Da fare

In ordine di priorità.

- [ ] **Provare un login completo con password.** Il prompt reale combacia (vedi la
      prova su `debminis`), ma nessuna password è mai stata inviata a un server: serve
      un account di prova. Controllare anche il caso di `HostKeyAlias` e
      `CanonicalizeHostname`: se il prompt non combacia, `ssh` chiede nel terminale,
      senza rischi.
- [ ] **Provare il Secret Service reale.** I test usano un finto `secret-tool`.
      Verificare salvataggio, lettura e cancellazione dal form.
- [ ] **Colori per connessione.** `apply_profile` è globale, quindi serve un override
      di palette sul singolo `TerminalBox`, senza toccare `apply_profile`.
- [ ] **Test automatici del pannello e del form.** Oggi sono stati verificati a mano
      con screenshot su Xvnc, mentre `tests/connections.rs` copre CLI e API di
      controllo. Il piano è pilotarli con `xdotool` o esporre un'operazione di prova.
- [ ] **Screenshot del README** con `scripts/capture-screenshots.sh`.
- [ ] **Voci «Connetti a…» nella palette e comando `/ssh nome`.** `Ctrl+Shift+K` e
      `oxterm connect` coprono già il bisogno. `PaletteAction` è una lista statica,
      quindi servono righe dinamiche.
- [ ] **Conteggi più precisi.** `●` conta solo i tab della finestra corrente. Un tab
      spostato in una finestra separata o in un altro processo perde l'associazione:
      `connection_id` non passa da `handover` né da `DetachedWindow`.
- [ ] **Posizione nel ripristino.** Le connessioni riaperte diventano tab nel pannello
      sinistro, anche se erano a destra.
- [ ] **Pannello nelle finestre separate.** Esiste solo in `MainWindow`.
- [ ] **Scorciatoie con il focus nel pannello.** `Ctrl+Shift+J` è gestita dal terminale
      e non chiude il pannello quando il focus è dentro: lì funzionano `Esc` e **Close**.
- [ ] **Decisione aperta: cronologia e notifiche per i comandi remoti.** Oggi Oxterm
      legge gli eventi solo dalla FIFO locale, apposta per non fidarsi dell'output.
      Un canale remoto richiede un progetto separato, e l'host remoto potrebbe
      falsificare gli eventi.

## Problemi trovati nel codice esistente

| Problema | Esito |
| --- | --- |
| `ssh -G` sincrono nel thread GUI | Corretto: thread e timeout di 2 s |
| `remote_attach` si blocca se `tmux_prefix_key` non è `b` | Corretto: il test ora usa una `HOME` temporanea |
| `remote_attach` lascia socket in `~/.config/oxterm/remote` se interrotto | Corretto per il futuro; restano 16 socket `oxterm-{attach,move}-*` di vecchie esecuzioni, innocui e cancellabili a mano |
| L'avviso di produzione guarda solo l'indirizzo | Corretto: per le connessioni salvate guarda anche il nome |
| Il titolo remoto sostituisce il nome della connessione | Corretto con `pin_tab_title` |
| Il controllo AUR compila il tag `v1.1.19`, che non contiene `oxterm-broker`, e fallisce anche su `master` dal commit `5f49453` | Corretto: la CI costruisce il commit corrente con la stessa ricetta |
| Lo script di integrazione remota non attiva cronologia e notifiche | Non corretto: serve una decisione, comportamento documentato nel manuale |
| Avvisi `clippy` su `terminal.rs`, `vi.rs`, `history.rs` e altri | Non toccati: sono di stile, e l'autore sta cambiando quei file per le prestazioni |
| `OpenSSH` usa la home di `passwd`, `dirs::home_dir` usa `$HOME` | Solo nota: con `HOME` diversa i due vedono `~/.ssh/config` diversi. Nei test usare `-F` |

## Come verificare

```bash
cargo test --locked --all-targets
cargo clippy --locked --all-targets
cargo fmt --check
cargo run --locked --bin oxterm-docgen   # non deve cambiare nulla

# Test con finestre reali, su un display virtuale:
Xvnc :77 -geometry 1400x900 -depth 24 -SecurityTypes None -rfbport 5977 -localhost &
DISPLAY=:77 OXTERM_GUI_TESTS=1 cargo test --locked \
  --test connections --test control_api --test gui_lifecycle -- --test-threads=1
```

Non usare `pkill -f` con un percorso del progetto: colpisce anche la shell che lo
esegue. Sul sistema dell'autore può girare il suo Oxterm, che non va interrotto.

### Prova manuale con un `sshd` vero

Eseguita il 2026-11-01 con `sshd -f cfg` non privilegiato su `127.0.0.1:22222`,
autenticazione a chiave, `HOME` temporanea e `extra_args: ["-F", cfg]`.

| Verifica | Esito |
| --- | --- |
| Sessione aperta con `oxterm connect`, comando eseguito sul server | Ok |
| Secondo tab sulla stessa connessione condivisa, un solo login in `sshd` | Ok |
| Tunnel `-L` in stato `listening`, `curl` attraverso il tunnel | Ok |
| Stop del tunnel: processo terminato, porta chiusa | Ok |
| Tunnel su porta occupata: errore esplicito | Ok |
| Host sconosciuto con `BatchMode`: `Host key verification failed` visibile | Ok |
| `kill -9` di Oxterm: `ssh -N` termina per `PDEATHSIG` | Ok |
| Domanda sulla chiave host attraverso `oxterm-askpass` in una PTY | Ok: arriva alla persona |
| Ripristino dopo `kill -9`: chiede prima di riconnettere | Ok |
| Pannello, ricerca e form su Xvnc | Ok, controllati con screenshot |

### Prova sul server reale `debminis`

Eseguita il 2026-11-01 con `ssh -p 2223 andres@debminis`, su richiesta dell'autore,
senza modificare nulla sul server. Autenticazione a chiave, OpenSSH 10.3 sul server.

| Verifica | Esito |
| --- | --- |
| `oxterm connect`: tab con nome fisso, comando eseguito sul server | Ok |
| Secondo tab sulla connessione condivisa | Ok |
| Tunnel `-L` verso la porta SSH remota: `listening`, banner `SSH-2.0-OpenSSH_10.3p1` letto attraverso il tunnel | Ok |
| Stop del tunnel | Ok |
| Pannello: conteggio di 2 tab e avviso sul nome `debminis-live` | Ok |
| `sftp -P 2223`: comando `pwd` | Ok |
| Prompt reale con `PreferredAuthentications=password`: `andres@debminis's password: ` | Combacia con `decide` per la destinazione di `ssh -G` |

Per catturare il prompt, un finto askpass ha registrato il testo e terminato `ssh`
prima di rispondere, quindi non è stata inviata nessuna password. Con
`keyboard-interactive` il server non ha mostrato prompt. Il formato
`(user@host) Password:` resta quindi non provato dal vivo.

## Hook e installazione

Il `post-commit` del repository reinstalla il binario in `PATH`. Durante questo
lavoro è stato escluso, per non mettere una build non ancora unita nel terminale
dell'autore. Il `pre-commit` con la build release è rimasto attivo. Dopo il merge,
il `post-merge` installa la nuova versione, compreso `oxterm-askpass` accanto a
`oxterm` (`scripts/refresh-installed.sh`).

## Registro

| Data | Lavoro |
| --- | --- |
| 2026-11-01 | Fasi 0-5, tranne i colori per connessione. Correzione dei test `remote_attach` e del controllo AUR. Prova su `debminis`. [PR #2](https://github.com/buzzqw/oxterm/pull/2). |
