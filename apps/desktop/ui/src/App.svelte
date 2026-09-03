<script lang="ts">
  import { onMount } from 'svelte';
  import {
    desktop,
    desktopError,
    onDownloadProgress,
    onPlatformError,
    onStateChange,
    type AppState,
    type DownloadProgress,
    type LexiconWorkspace,
    type LocalProviderStatus,
    type ModelCatalogItem,
    type RecordingSettings,
    type RemoteConnectionTest,
    type RemoteProviderWorkspace,
    type RuntimeSnapshot,
    type Transcript,
    type WindowsIntegrationSettings
  } from './lib/api/desktop';
  import { labelForState } from './lib/status';
  import { workspaceShellFor } from './lib/workspace-shell';

  type Page = 'home' | 'models' | 'history' | 'lexicon' | 'providers' | 'settings';
  type SetupPath = 'local' | 'remote';

  let page: Page = 'home';
  let setupPath: SetupPath = 'local';
  let snapshot: RuntimeSnapshot | null = null;
  let catalog: ModelCatalogItem[] = [];
  let history: Transcript[] = [];
  let historyQuery = '';
  let lexicon: LexiconWorkspace | null = null;
  let providerStatus: LocalProviderStatus | null = null;
  let remoteProviders: RemoteProviderWorkspace | null = null;
  let remoteTest: RemoteConnectionTest | null = null;
  let result: Transcript | null = null;
  let state: AppState = 'idle';
  let errorMessage: string | null = null;
  let notice: string | null = null;
  let busyAction: string | null = null;
  let downloads = new Map<string, DownloadProgress>();
  let statusTimer: number | undefined;
  let settings: RecordingSettings = {
    deviceId: null,
    mode: 'toggle',
    maxDurationSeconds: 180,
    vad: { enabled: true, aggressiveness: 2, minimumSpeechMs: 180, endSilenceMs: 1200 }
  };
  let windowsSettings: WindowsIntegrationSettings = {
    hotkey: { control: true, alt: true, shift: false, key: 'space' },
    autoPaste: false,
    restoreClipboardAfterPaste: true
  };
  let settingsLoaded = false;
  let profileName = '';
  let entryCanonical = '';
  let entryVariant = '';
  let entryLanguage = '';
  let entryCategory = '';
  let entryPriority = 50;
  let entryProfileId: string | null = null;
  let entryCaseSensitive = false;
  let remoteDisplayName = 'Eigener Rechner';
  let remoteEndpoint = '';
  let remoteBearerToken = '';
  let remoteSelectedModelId = '';
  let remoteAllowHttpLoopback = false;
  let remoteConnectTimeoutMs = 5000;
  let remoteRequestTimeoutMs = 600000;

  $: workspaceShell = workspaceShellFor(snapshot?.onboarding);
  $: currentRecording = snapshot?.recording ?? null;
  $: isWorking = Boolean(snapshot?.processing || currentRecording !== null || busyAction !== null);
  $: hotkeyLabel = [
    windowsSettings.hotkey.control ? 'Ctrl' : '',
    windowsSettings.hotkey.alt ? 'Alt' : '',
    windowsSettings.hotkey.shift ? 'Shift' : '',
    'Leertaste'
  ].filter(Boolean).join('+');

  onMount(() => {
    void refresh();
    const unlisteners: Array<() => void> = [];
    void onDownloadProgress((progress) => {
      if (progress.apiVersion !== 1) return;
      downloads = new Map(downloads).set(progress.modelId, progress);
    }).then((unlisten) => unlisteners.push(unlisten));
    void onStateChange((event) => {
      if (event.apiVersion !== 1) return;
      state = event.state;
      if (event.detail) errorMessage = event.detail;
      if (event.state === 'recording') beginStatusPolling();
      if (event.state === 'failed' || event.state === 'awaiting_injection_confirmation' || event.state === 'completed') {
        stopStatusPolling();
      }
    }).then((unlisten) => unlisteners.push(unlisten));
    void onPlatformError((event) => {
      if (event.apiVersion === 1) errorMessage = event.message;
    }).then((unlisten) => unlisteners.push(unlisten));
    return () => {
      stopStatusPolling();
      unlisteners.forEach((unlisten) => unlisten());
    };
  });

  async function refresh(loadCatalog = true) {
    try {
      const next = await desktop.snapshot();
      snapshot = next;
      result ??= next.lastTranscript;
      if (!settingsLoaded) {
        settings = {
          ...next.recordingSettings,
          deviceId: next.inputDevices.some((device) => device.id === next.recordingSettings.deviceId)
            ? next.recordingSettings.deviceId
            : next.inputDevices.find((device) => device.isDefault)?.id ?? null
        };
        windowsSettings = next.windowsIntegration;
        settingsLoaded = true;
      }
      if (next.recordingSettingsError || next.windowsIntegrationError || next.hotkeyError) {
        errorMessage = next.recordingSettingsError ?? next.windowsIntegrationError ?? next.hotkeyError;
      }
      if (loadCatalog && next.manifestAvailable) catalog = await desktop.modelCatalog();
      if (page === 'history') history = await desktop.recentTranscripts();
      if (page === 'lexicon') lexicon = await desktop.lexiconWorkspace();
      if (page === 'providers') {
        [providerStatus, remoteProviders] = await Promise.all([
          desktop.localProviderStatus(),
          desktop.remoteProviderWorkspace()
        ]);
      }
      if (next.activeProvider) state = next.recording ? 'recording' : next.processing ? 'transcribing' : 'idle';
    } catch (error) {
      errorMessage = desktopError(error).message;
    }
  }

  async function installAndActivate(model: ModelCatalogItem) {
    errorMessage = null;
    busyAction = model.id;
    try {
      await desktop.installModel(model.id);
      await desktop.activateModel(model.id);
      downloads = new Map(downloads);
      downloads.delete(model.id);
      notice = `${model.displayName} ist verifiziert installiert und aktiv.`;
      page = 'home';
      await refresh();
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function activate(model: ModelCatalogItem) {
    errorMessage = null;
    busyAction = model.id;
    try {
      await desktop.activateModel(model.id);
      notice = `${model.displayName} ist jetzt das aktive CPU-Modell.`;
      await refresh();
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function cancelDownload(modelId: string) {
    try {
      await desktop.cancelInstall(modelId);
      notice = 'Download wird angehalten. Der Teilstand bleibt für eine sichere Fortsetzung erhalten.';
    } catch (error) { errorMessage = desktopError(error).message; }
  }

  async function removeModel(model: ModelCatalogItem) {
    if (!window.confirm(`${model.displayName} wirklich löschen? Die Modelldatei wird entfernt.`)) return;
    errorMessage = null;
    busyAction = model.id;
    try {
      await desktop.deleteModel(model.id, true);
      notice = `${model.displayName} wurde gelöscht.`;
      await refresh();
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function beginRecording() {
    errorMessage = null;
    notice = null;
    busyAction = 'record';
    try {
      const recording = await desktop.startRecording({ settings, language: null });
      snapshot = snapshot ? { ...snapshot, recording } : snapshot;
      state = 'recording';
      beginStatusPolling();
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function finishRecording() {
    errorMessage = null;
    busyAction = 'transcribe';
    stopStatusPolling();
    try {
      state = 'finalizing_audio';
      const transcript = await desktop.stopRecording();
      result = transcript;
      state = transcript.injectionOutcome === 'inserted' ? 'completed' : 'awaiting_injection_confirmation';
      snapshot = snapshot ? { ...snapshot, recording: null, processing: false, lastTranscript: transcript } : snapshot;
      history = [transcript, ...history.filter((entry) => entry.id !== transcript.id)];
      notice = transcript.injectionOutcome === 'inserted'
        ? 'Text wurde nur im unveränderten Zielkontext eingefügt.'
        : 'Text ist bereit. Er wird erst nach deinem Klick kopiert.';
    } catch (error) {
      errorMessage = desktopError(error).message;
      state = 'failed';
      await refresh(false);
    } finally { busyAction = null; }
  }

  async function cancelCurrent() {
    try {
      await desktop.cancelCurrentJob();
      stopStatusPolling();
      state = 'idle';
      notice = 'Aufnahme beziehungsweise Transkription wurde abgebrochen.';
      await refresh(false);
    } catch (error) { errorMessage = desktopError(error).message; }
  }

  async function copyResult() {
    if (!result) return;
    errorMessage = null;
    busyAction = 'copy';
    try {
      await desktop.copyTranscript(result.id);
      result = { ...result, injectionOutcome: 'copied_to_clipboard' };
      notice = 'Endtext wurde in die Zwischenablage kopiert.';
      state = 'completed';
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function pasteOriginal() {
    if (!result) return;
    errorMessage = null;
    busyAction = 'paste';
    try {
      result = await desktop.pasteTranscriptToOriginal(result.id);
      notice = result.injectionOutcome === 'inserted'
        ? 'Text wurde im gespeicherten Originalfenster eingefügt.'
        : 'Das Ziel war nicht mehr sicher. Der Text wurde nur kopiert.';
      state = result.injectionOutcome === 'inserted' ? 'completed' : 'awaiting_injection_confirmation';
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function saveWindowsSettings() {
    busyAction = 'windows-settings';
    try {
      await desktop.saveWindowsIntegrationSettings(windowsSettings);
      notice = 'Windows-Einstellungen sind lokal gespeichert. Eine geänderte Tastenkombination wird beim nächsten App-Start registriert.';
      await refresh(false);
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function searchHistory() {
    try {
      history = await desktop.searchTranscripts(historyQuery);
    } catch (error) { errorMessage = desktopError(error).message; }
  }

  async function revertCorrection(transcriptId: string, correctionId: string) {
    busyAction = `correction-${correctionId}`;
    try {
      const updated = await desktop.revertTranscriptCorrection(transcriptId, correctionId);
      if (result?.id === updated.id) result = updated;
      history = history.map((item) => item.id === updated.id ? updated : item);
      notice = 'Die Nachkorrektur wurde aus dem unveränderten Rohtext neu berechnet.';
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function refreshLexicon() {
    try { lexicon = await desktop.lexiconWorkspace(); }
    catch (error) { errorMessage = desktopError(error).message; }
  }

  async function addProfile() {
    if (!profileName.trim()) return;
    busyAction = 'lexicon-profile';
    try {
      lexicon = await desktop.saveLexiconProfile({ id: null, name: profileName, enabled: true });
      profileName = '';
      notice = 'Lexikonprofil wurde lokal gespeichert.';
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function selectActiveProfile(profileId: string | null) {
    busyAction = 'lexicon-active-profile';
    try {
      lexicon = await desktop.setActiveLexiconProfile(profileId);
      entryProfileId = profileId;
      notice = profileId ? 'Profil ist für neue Aufnahmen aktiv.' : 'Nur globale Begriffe sind aktiv.';
      await refresh(false);
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function addLexiconEntry() {
    if (!entryCanonical.trim()) return;
    busyAction = 'lexicon-entry';
    try {
      lexicon = await desktop.saveLexiconEntry({
        canonicalText: entryCanonical,
        language: entryLanguage.trim() || null,
        category: entryCategory.trim() || null,
        priority: Number(entryPriority),
        enabled: true,
        profileId: entryProfileId,
        variants: entryVariant
          .split(/\r?\n/u)
          .map((variantText) => variantText.trim())
          .filter(Boolean)
          .map((variantText) => ({ variantText, matchMode: entryCaseSensitive ? 'exact_case_sensitive' : 'exact_casefold' }))
      });
      entryCanonical = ''; entryVariant = ''; entryLanguage = ''; entryCategory = '';
      notice = 'Begriff und Varianten wurden ohne Fuzzy-Regeln gespeichert.';
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function deleteProfile(profileId: string) {
    if (!window.confirm('Profil und alle zugehörigen Begriffe wirklich löschen?')) return;
    try { lexicon = await desktop.deleteLexiconProfile(profileId); }
    catch (error) { errorMessage = desktopError(error).message; }
  }

  async function deleteEntry(entryId: string) {
    if (!window.confirm('Begriff und Varianten wirklich löschen?')) return;
    try { lexicon = await desktop.deleteLexiconEntry(entryId); }
    catch (error) { errorMessage = desktopError(error).message; }
  }

  async function importLexiconFile(event: Event) {
    const file = (event.currentTarget as HTMLInputElement).files?.[0];
    if (!file) return;
    busyAction = 'lexicon-import';
    try {
      const format = file.name.toLowerCase().endsWith('.csv') ? 'csv' : 'json';
      const report = await desktop.importLexicon(format, await file.text());
      lexicon = await desktop.lexiconWorkspace();
      notice = `Import: ${report.addedEntries} Begriffe, ${report.addedVariants} Varianten, ${report.skipped} übersprungen.`;
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; (event.currentTarget as HTMLInputElement).value = ''; }
  }

  async function exportLexicon(format: 'csv' | 'json') {
    try {
      const exported = await desktop.exportLexicon(format);
      const blob = new Blob([exported.content], { type: format === 'csv' ? 'text/csv;charset=utf-8' : 'application/json' });
      const href = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = href; anchor.download = `free-whisper-lexicon.${format}`; anchor.click();
      URL.revokeObjectURL(href);
      notice = `Lexikon wurde als ${format.toUpperCase()} exportiert.`;
    } catch (error) { errorMessage = desktopError(error).message; }
  }

  async function testRemoteProvider() {
    errorMessage = null;
    remoteTest = null;
    busyAction = 'remote-test';
    try {
      remoteTest = await desktop.testRemoteProvider({
        endpoint: remoteEndpoint,
        bearerToken: remoteBearerToken,
        developerAllowHttpLoopback: remoteAllowHttpLoopback,
        connectTimeoutMs: Number(remoteConnectTimeoutMs),
        requestTimeoutMs: Number(remoteRequestTimeoutMs)
      });
      const firstReady = remoteTest.models.find((model) => model.installed && model.ready);
      if (!remoteSelectedModelId && firstReady) remoteSelectedModelId = firstReady.id;
      notice = 'Verbindung, TLS/Loopback-Regel, Authentifizierung und Fähigkeiten wurden geprüft. Das Token wurde noch nicht gespeichert.';
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function saveRemoteProvider() {
    if (!remoteTest || !remoteSelectedModelId) {
      errorMessage = 'Bitte teste die Verbindung und wähle ein bereites Modell, bevor du speicherst.';
      return;
    }
    busyAction = 'remote-save';
    try {
      remoteProviders = await desktop.saveRemoteProvider({
        id: null,
        displayName: remoteDisplayName,
        endpoint: remoteEndpoint,
        bearerToken: remoteBearerToken,
        selectedModelId: remoteSelectedModelId,
        developerAllowHttpLoopback: remoteAllowHttpLoopback,
        connectTimeoutMs: Number(remoteConnectTimeoutMs),
        requestTimeoutMs: Number(remoteRequestTimeoutMs),
        enabled: true,
        activateAfterSave: true
      });
      remoteBearerToken = '';
      notice = 'Remote-Provider wurde nach erfolgreichem Test aktiviert. Sein Token liegt ausschließlich im Windows Credential Manager.';
      await refresh(false);
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function activateRemoteProvider(profileId: string) {
    busyAction = `remote-activate-${profileId}`;
    try {
      remoteProviders = await desktop.activateRemoteProvider(profileId);
      notice = 'Der eigene Remote-Worker ist aktiv. Audio wird bei einer Aufnahme bewusst an diesen Endpunkt gesendet.';
      await refresh(false);
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function useLocalProvider() {
    busyAction = 'local-provider';
    try {
      remoteProviders = await desktop.useLocalProvider();
      notice = 'Lokaler CPU-Worker ist wieder aktiv. Neue Aufnahmen verlassen den Rechner nicht.';
      await refresh(false);
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  async function deleteRemoteProvider(profileId: string) {
    if (!window.confirm('Remote-Provider inklusive gespeicherter Windows-Anmeldung wirklich entfernen?')) return;
    busyAction = `remote-delete-${profileId}`;
    try {
      remoteProviders = await desktop.deleteRemoteProvider(profileId, true);
      notice = 'Remote-Provider und seine gespeicherte Anmeldung wurden entfernt.';
      await refresh(false);
    } catch (error) { errorMessage = desktopError(error).message; }
    finally { busyAction = null; }
  }

  function beginStatusPolling() {
    if (statusTimer !== undefined) return;
    statusTimer = window.setInterval(async () => {
      try {
        const recording = await desktop.recordingStatus();
        if (snapshot) snapshot = { ...snapshot, recording };
        if ((recording?.vadEndDetected && settings.mode === 'toggle') || recording?.maxDurationReached) {
          stopStatusPolling();
          void finishRecording();
        } else if (!recording) stopStatusPolling();
      } catch {
        stopStatusPolling();
        errorMessage = 'Der Aufnahmestatus konnte nicht mehr gelesen werden.';
      }
    }, 180);
  }

  function stopStatusPolling() {
    if (statusTimer !== undefined) window.clearInterval(statusTimer);
    statusTimer = undefined;
  }

  function progressFor(modelId: string) { return downloads.get(modelId); }
  function percent(progress: DownloadProgress | undefined) {
    return !progress || progress.totalBytes === 0 ? 0 : Math.min(100, Math.round((progress.downloadedBytes / progress.totalBytes) * 100));
  }
  function formatBytes(bytes: number) {
    return bytes < 1024 * 1024 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1024 / 1024).toFixed(bytes > 1024 * 1024 * 1024 ? 1 : 0)} MB`;
  }
  function formatDuration(milliseconds: number) {
    const seconds = Math.floor(milliseconds / 1000);
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  }
  function presetLabel(preset: ModelCatalogItem['preset']) { return { fast: 'Schnell', balanced: 'Ausgewogen', high_quality: 'Hohe Qualität' }[preset]; }
  function outcomeLabel(transcript = result) {
    if (!transcript) return 'Nicht kopiert';
    if (transcript.injectionOutcome === 'inserted') return 'Eingefügt';
    if (transcript.injectionOutcome === 'copied_to_clipboard') return 'In Zwischenablage kopiert';
    if (typeof transcript.injectionOutcome === 'object' && 'failed_with_reason' in transcript.injectionOutcome) return 'Übergabe fehlgeschlagen';
    return 'Wartet auf Bestätigung';
  }
</script>

<svelte:head><title>free-whisper – lokale Transkription</title></svelte:head>

{#if workspaceShell === 'loading'}
  <main class="onboarding-shell" aria-live="polite">
    <section class="onboarding-panel onboarding-loading"><div class="brand"><img src="/brand/free-whisper-signal-master.png" alt="" /><span><strong>free-whisper</strong><small>LOCAL TRANSCRIPTION</small></span></div><div class="loading"><i></i> Systemstatus wird lokal geprüft …</div></section>
  </main>
{:else if workspaceShell === 'onboarding'}
  <main class="onboarding-shell" aria-live="polite">
    <section class="onboarding-panel">
      <header class="onboarding-topbar"><div class="brand"><img src="/brand/free-whisper-signal-master.png" alt="" /><span><strong>free-whisper</strong><small>LOCAL TRANSCRIPTION</small></span></div><span class="onboarding-state"><i></i> Einrichtung erforderlich</span></header>

      {#if errorMessage}<div class="message message-error" role="alert"><div><b>Aktion braucht Aufmerksamkeit</b><span>{errorMessage}</span></div><button aria-label="Fehlermeldung schließen" on:click={() => (errorMessage = null)}>×</button></div>{/if}
      {#if notice}<div class="message message-info"><div><b>Hinweis</b><span>{notice}</span></div><button aria-label="Hinweis schließen" on:click={() => (notice = null)}>×</button></div>{/if}

      <section class="onboarding-content" aria-labelledby="setup-title">
        <div class="setup-intro"><span class="workspace-kicker">SCHRITT 1 VON 1 · AUSFÜHRUNG WÄHLEN</span><h1 id="setup-title">Dein Arbeitsbereich<br />beginnt lokal.</h1><p>Wähle, wo deine Transkriptionen laufen sollen. Downloads und Verbindungen starten ausschließlich nach deinem Klick.</p></div>
        <div class="setup-choice-grid" role="tablist" aria-label="Ausführungsort wählen">
          <button class:active={setupPath === 'local'} role="tab" aria-selected={setupPath === 'local'} on:click={() => { setupPath = 'local'; remoteTest = null; }}><span class="choice-icon">⌁</span><strong>Auf diesem PC</strong><small>Empfohlen · Offline · CPU</small></button>
          <button class:active={setupPath === 'remote'} role="tab" aria-selected={setupPath === 'remote'} on:click={() => { setupPath = 'remote'; }}><span class="choice-icon">↗</span><strong>Eigener Rechner</strong><small>Optional · eigener Remote-Worker</small></button>
        </div>

        {#if setupPath === 'local'}
          <div class="setup-path-heading"><div><span class="workspace-kicker">LOKALER CPU-WORKER</span><h2>Modell auswählen</h2><p>Jede Datei wird vor der Aktivierung gegen den signierten Katalog und ihre Prüfsumme validiert.</p></div><div class="heading-note">KEINE CLOUD<br /><b>Audio bleibt hier</b></div></div>
          {#if !snapshot?.manifestAvailable}
            <div class="integrity-card" role="alert"><div class="warning-mark">!</div><div><b>Signierter Modellkatalog fehlt</b><p>{snapshot?.manifestError ?? 'Der Modellkatalog ist nicht verfügbar.'}</p><small>Die Anwendung startet absichtlich keinen unsignierten Download.</small></div></div>
          {:else}
            <div class="model-grid">
              {#each catalog as model}
                {@const download = progressFor(model.id)}
                <article class:active-model={model.active} class="model-card">
                  <div class="model-card-top"><span class="pill">{presetLabel(model.preset)}</span>{#if model.active}<span class="pill pill-active">Aktiv</span>{/if}</div>
                  <h2>{model.displayName}</h2><p class="model-id">{model.id}</p>
                  <dl><div><dt>Download</dt><dd>{formatBytes(model.sizeBytes)}</dd></div><div><dt>RAM</dt><dd>ca. {formatBytes(model.estimatedRamBytes)}</dd></div><div><dt>Backend</dt><dd>{model.backend.toUpperCase()} · {model.quantization}</dd></div></dl>
                  <div class="model-links"><a href={model.licenseUrl} target="_blank" rel="noreferrer">{model.license}</a><a href={model.sourceUrl} target="_blank" rel="noreferrer">Herkunft</a></div>
                  {#if download}
                    <div class="progress-wrap"><div><b>{percent(download)} %</b><span>{formatBytes(download.bytesPerSecond)}/s</span></div><progress max="100" value={percent(download)}></progress><small>{download.estimatedRemainingSeconds === null ? 'Restzeit wird berechnet …' : `noch etwa ${download.estimatedRemainingSeconds}s`}</small><button class="button-link" on:click={() => void cancelDownload(model.id)}>Download abbrechen</button></div>
                  {:else if !model.installed}
                    <button class="button button-primary" disabled={busyAction === model.id} on:click={() => void installAndActivate(model)}>{busyAction === model.id ? 'Wird vorbereitet …' : 'Installieren & aktivieren'}</button>
                  {:else if !model.active}
                    <div class="button-row"><button class="button button-primary" disabled={isWorking} on:click={() => void activate(model)}>Aktivieren</button><button class="button-link danger" disabled={isWorking} on:click={() => void removeModel(model)}>Löschen</button></div>
                  {:else}<div class="ready-line"><span>✓</span> Verifiziert und einsatzbereit</div>{/if}
                </article>
              {/each}
            </div>
          {/if}
        {:else}
          <section class="onboarding-remote remote-provider-card" aria-labelledby="onboarding-remote-title"><div><span class="workspace-kicker">EIGENER RECHNER</span><h2 id="onboarding-remote-title">Remote-Worker verbinden</h2><p>HTTPS ist Pflicht. Nur für einen expliziten Loopback-Entwicklertest kann HTTP aktiviert werden. Der Bearer-Token wird erst nach einem erfolgreichen Test im Windows Credential Manager abgelegt.</p></div><div class="remote-form"><label><span>Name</span><input bind:value={remoteDisplayName} maxlength="80" placeholder="z. B. Arbeitsrechner" /></label><label class="remote-wide"><span>HTTPS-Endpunkt</span><input bind:value={remoteEndpoint} inputmode="url" placeholder="https://worker.example.net" /></label><label class="remote-wide"><span>Bearer-Token</span><input type="password" bind:value={remoteBearerToken} autocomplete="off" placeholder="wird nie in SQLite oder Logs gespeichert" /></label><label><span>Connect-Timeout (ms)</span><input type="number" min="1000" max="60000" bind:value={remoteConnectTimeoutMs} /></label><label><span>Request-Timeout (ms)</span><input type="number" min="5000" max="600000" bind:value={remoteRequestTimeoutMs} /></label><label class="checkbox remote-wide"><input type="checkbox" bind:checked={remoteAllowHttpLoopback} /> Entwicklungsmodus: HTTP nur für 127.0.0.1/localhost erlauben</label><div class="button-row remote-wide"><button class="button button-secondary" disabled={busyAction === 'remote-test'} on:click={() => void testRemoteProvider()}>{busyAction === 'remote-test' ? 'Test läuft …' : 'Verbindung testen'}</button>{#if remoteTest}<button class="button button-primary" disabled={busyAction === 'remote-save' || !remoteTest.healthy || !remoteTest.remoteExecution || !remoteSelectedModelId} on:click={() => void saveRemoteProvider()}>{busyAction === 'remote-save' ? 'Wird sicher gespeichert …' : 'Speichern & aktivieren'}</button>{/if}</div></div>
            {#if remoteTest}<div class="remote-test-result"><div class="provider-status-heading"><div><span class:danger={!remoteTest.healthy || !remoteTest.remoteExecution} class="pill pill-active">{remoteTest.healthy && remoteTest.remoteExecution ? 'VERBINDUNG BEREIT' : 'NICHT KOMPATIBEL'}</span><p>{remoteTest.detail} · max. {formatBytes(remoteTest.maxRequestBytes)} · {formatDuration(remoteTest.maxAudioDurationMs)} Audio</p></div></div><label><span>Bereites Worker-Modell</span><select bind:value={remoteSelectedModelId}><option value="" disabled>Modell auswählen</option>{#each remoteTest.models as model}<option value={model.id} disabled={!model.installed || !model.ready}>{model.displayName} · {model.id}{model.ready ? '' : ' (nicht bereit)'}</option>{/each}</select></label><p class="muted">Spracherkennung: {remoteTest.languageDetection ? 'ja' : 'nein'} · Wortzeitstempel: {remoteTest.wordTimestamps ? 'ja' : 'nein'}. Ein Test speichert den Token noch nicht.</p></div>{/if}
          </section>
        {/if}
      </section>
    </section>
  </main>
{:else}
<main>
  <aside class="sidebar" aria-label="Hauptnavigation">
    <div class="brand">
      <img src="/brand/free-whisper-signal-master.png" alt="" />
      <span><strong>free-whisper</strong><small>LOCAL TRANSCRIPTION</small></span>
    </div>

    <nav aria-label="Arbeitsbereiche">
      <button class:active={page === 'home'} on:click={() => (page = 'home')}><span class="nav-icon">●</span> Aufnahme</button>
      <button class:active={page === 'models'} on:click={() => (page = 'models')}><span class="nav-icon">◇</span> Modelle</button>
      <button class:active={page === 'history'} on:click={() => { page = 'history'; void refresh(false); }}><span class="nav-icon">↺</span> Verlauf</button>
      <button class:active={page === 'lexicon'} on:click={() => { page = 'lexicon'; void refresh(false); }}><span class="nav-icon">✦</span> Lexikon</button>
      <button class:active={page === 'providers'} on:click={() => { page = 'providers'; void refresh(false); }}><span class="nav-icon">◌</span> Provider</button>
      <button class:active={page === 'settings'} on:click={() => (page = 'settings')}><span class="nav-icon">⚙</span> Windows & Sicherheit</button>
    </nav>

    <div class="sidebar-bottom">
      <div class="presence"><span class:recording={state === 'recording'}></span><b>{labelForState(state)}</b></div>
      <p>Offline · CPU · keine Telemetrie</p>
      <small>v{snapshot?.appVersion ?? '…'}</small>
    </div>
  </aside>

  <section class="workspace" aria-live="polite">
    <header class="topbar">
      <div><span class="workspace-kicker">PERSÖNLICHER ARBEITSBEREICH</span><strong>{snapshot?.activeProvider?.displayName ?? 'Einrichtung erforderlich'}</strong></div>
      <div class="topbar-status"><span class="status-dot"></span>{snapshot?.activeProvider ? `${snapshot.activeProvider.kind === 'remote' ? 'EIGENER REMOTE-WORKER' : snapshot.activeProvider.backend.toUpperCase()} · ${snapshot.activeProvider.modelId}` : 'Kein Provider aktiv'}{#if (snapshot?.queueDepth ?? 0) > 0} · 1 wartet{/if}</div>
    </header>

    {#if errorMessage}<div class="message message-error" role="alert"><div><b>Aktion braucht Aufmerksamkeit</b><span>{errorMessage}</span></div><button aria-label="Fehlermeldung schließen" on:click={() => (errorMessage = null)}>×</button></div>{/if}
    {#if notice}<div class="message message-info"><div><b>Hinweis</b><span>{notice}</span></div><button aria-label="Hinweis schließen" on:click={() => (notice = null)}>×</button></div>{/if}

    {#if !snapshot}
      <div class="loading"><i></i> Systemstatus wird lokal geprüft …</div>
    {:else if page === 'models'}
      <section class="page models-page" aria-labelledby="models-title">
        <div class="page-heading"><div><span class="workspace-kicker">EINRICHTUNG</span><h1 id="models-title">Modell auswählen</h1><p>Ein Download beginnt ausschließlich nach deinem Klick und wird vor der Aktivierung kryptografisch geprüft.</p></div><div class="heading-note">CPU-Worker<br /><b>lokal & isoliert</b></div></div>
        {#if !snapshot.manifestAvailable}
          <div class="integrity-card" role="alert"><div class="warning-mark">!</div><div><b>Signierter Modellkatalog fehlt</b><p>{snapshot.manifestError ?? 'Der Modellkatalog ist nicht verfügbar.'}</p><small>Die Anwendung startet absichtlich keinen unsignierten Download.</small></div></div>
        {:else}
          <div class="model-grid">
            {#each catalog as model}
              {@const download = progressFor(model.id)}
              <article class:active-model={model.active} class="model-card">
                <div class="model-card-top"><span class="pill">{presetLabel(model.preset)}</span>{#if model.active}<span class="pill pill-active">Aktiv</span>{/if}</div>
                <h2>{model.displayName}</h2><p class="model-id">{model.id}</p>
                <dl><div><dt>Download</dt><dd>{formatBytes(model.sizeBytes)}</dd></div><div><dt>RAM</dt><dd>ca. {formatBytes(model.estimatedRamBytes)}</dd></div><div><dt>Backend</dt><dd>{model.backend.toUpperCase()} · {model.quantization}</dd></div></dl>
                <div class="model-links"><a href={model.licenseUrl} target="_blank" rel="noreferrer">{model.license}</a><a href={model.sourceUrl} target="_blank" rel="noreferrer">Herkunft</a></div>
                {#if download}
                  <div class="progress-wrap"><div><b>{percent(download)} %</b><span>{formatBytes(download.bytesPerSecond)}/s</span></div><progress max="100" value={percent(download)}></progress><small>{download.estimatedRemainingSeconds === null ? 'Restzeit wird berechnet …' : `noch etwa ${download.estimatedRemainingSeconds}s`}</small><button class="button-link" on:click={() => cancelDownload(model.id)}>Download abbrechen</button></div>
                {:else if !model.installed}
                  <button class="button button-primary" disabled={busyAction === model.id} on:click={() => installAndActivate(model)}>{busyAction === model.id ? 'Wird vorbereitet …' : 'Installieren & aktivieren'}</button>
                {:else if !model.active}
                  <div class="button-row"><button class="button button-primary" disabled={isWorking} on:click={() => activate(model)}>Aktivieren</button><button class="button-link danger" disabled={isWorking} on:click={() => removeModel(model)}>Löschen</button></div>
                {:else}<div class="ready-line"><span>✓</span> Verifiziert und einsatzbereit</div>{/if}
              </article>
            {/each}
          </div>
        {/if}
      </section>
    {:else if page === 'history'}
      <section class="page" aria-labelledby="history-title">
        <div class="page-heading"><div><span class="workspace-kicker">LOKALER VERLAUF</span><h1 id="history-title">Transkripte</h1><p>Audiodateien werden nicht gespeichert. Ergebnis, Korrekturen und Laufzeiten liegen lokal in SQLite.</p></div></div>
        <label class="search-field"><span>Verlauf durchsuchen</span><input bind:value={historyQuery} maxlength="512" placeholder="Text suchen – Sonderzeichen bleiben wörtlich" on:input={() => void searchHistory()} /></label>
        {#if history.length === 0}<div class="empty-state">Noch keine Transkription. Starte auf der Aufnahmeseite.</div>{/if}
        <div class="history-list">{#each history as transcript}<button class="history-row" on:click={() => { result = transcript; page = 'home'; }}><time>{new Date(transcript.createdAt).toLocaleString('de-DE')}</time><strong>{transcript.finalText || 'Leeres Ergebnis'}</strong><span>{outcomeLabel(transcript)} · {formatDuration(transcript.processingMs)}</span></button>{/each}</div>
      </section>
    {:else if page === 'lexicon'}
      <section class="page lexicon-page" aria-labelledby="lexicon-title">
        <div class="page-heading"><div><span class="workspace-kicker">PERSÖNLICHES LEXIKON</span><h1 id="lexicon-title">Begriffe, die zählen.</h1><p>Hinweise und Nachkorrekturen bleiben bewusst konservativ: nur aktive Profile und exakt hinterlegte Varianten.</p></div></div>
        {#if !lexicon}<div class="loading"><i></i> Lexikon wird lokal geladen …</div>{:else}
          <div class="lexicon-layout">
            <section class="settings-card lexicon-profiles"><div class="setting-title"><div><h2>Aktives Profil</h2><p>Globale Begriffe gelten immer; ein Profil begrenzt zusätzliche Hinweise für neue Aufnahmen.</p></div></div>
              <select value={lexicon.activeProfileId ?? ''} on:change={(event) => void selectActiveProfile((event.currentTarget as HTMLSelectElement).value || null)}><option value="">Nur global</option>{#each lexicon.profiles.filter((profile) => profile.enabled) as profile}<option value={profile.id}>{profile.name}</option>{/each}</select>
              <div class="inline-form"><input bind:value={profileName} placeholder="Neues Profil, z. B. Arbeit" on:keydown={(event) => event.key === 'Enter' && void addProfile()} /><button class="button button-secondary" disabled={busyAction === 'lexicon-profile'} on:click={addProfile}>Profil anlegen</button></div>
              <div class="profile-list">{#each lexicon.profiles as profile}<div><span class:profile-disabled={!profile.enabled}>{profile.name}{#if !profile.enabled} · deaktiviert{/if}</span><button class="button-link danger" on:click={() => void deleteProfile(profile.id)}>Löschen</button></div>{/each}</div>
            </section>
            <section class="settings-card lexicon-entry-form"><div class="setting-title"><div><h2>Begriff hinzufügen</h2><p>Mehrere Varianten sind möglich: jede Zeile wird als exakte, sichere Regel gespeichert.</p></div></div>
              <div class="field-grid"><label><span>Kanonischer Begriff</span><input bind:value={entryCanonical} placeholder="z. B. free-whisper" /></label><label><span>Profil</span><select bind:value={entryProfileId}><option value={null}>Global</option>{#each lexicon.profiles.filter((profile) => profile.enabled) as profile}<option value={profile.id}>{profile.name}</option>{/each}</select></label><label><span>Sprache</span><input bind:value={entryLanguage} placeholder="de" /></label><label><span>Kategorie</span><input bind:value={entryCategory} placeholder="Produkt" /></label><label><span>Priorität (0–100)</span><input type="number" min="0" max="100" bind:value={entryPriority} /></label><label class="checkbox compact"><input type="checkbox" bind:checked={entryCaseSensitive} /> Varianten unterscheiden Groß-/Kleinschreibung</label></div>
              <label><span>Varianten</span><textarea bind:value={entryVariant} rows="3" placeholder={'z. B.\nfree whisper\nfreewhisper'}></textarea></label>
              <button class="button button-primary" disabled={busyAction === 'lexicon-entry'} on:click={addLexiconEntry}>Sicher speichern</button>
            </section>
          </div>
          <section class="lexicon-transfer"><div><b>Import & Export</b><p>Import prüft alle Daten vor der atomaren Speicherung. Duplikate werden getrennt berichtet.</p></div><label class="button button-secondary file-button">CSV oder JSON importieren<input type="file" accept=".csv,.json,text/csv,application/json" on:change={importLexiconFile} /></label><div class="button-row"><button class="button button-secondary" on:click={() => void exportLexicon('csv')}>CSV exportieren</button><button class="button button-secondary" on:click={() => void exportLexicon('json')}>JSON exportieren</button></div></section>
          <section class="lexicon-entries" aria-label="Gespeicherte Lexikonbegriffe">{#if lexicon.entries.length === 0}<div class="empty-state">Noch keine Begriffe. Varianten werden niemals automatisch aus einer ASR-Antwort gelernt.</div>{/if}{#each lexicon.entries as entry}<article><div><span class="pill">{entry.profileId ? lexicon.profiles.find((profile) => profile.id === entry.profileId)?.name ?? 'Profil' : 'Global'}</span><h2>{entry.canonicalText}</h2><p>{entry.language ?? 'sprachübergreifend'}{#if entry.category} · {entry.category}{/if} · Priorität {entry.priority}</p><div class="variant-list">{#each entry.variants as variant}<span>{variant.variantText} <small>{variant.matchMode === 'exact_case_sensitive' ? 'Aa' : 'aa'}</small></span>{/each}</div></div><button class="button-link danger" on:click={() => void deleteEntry(entry.id)}>Löschen</button></article>{/each}</section>
        {/if}
      </section>
    {:else if page === 'providers'}
      <section class="page providers-page" aria-labelledby="providers-title">
        <div class="page-heading"><div><span class="workspace-kicker">AUSFÜHRUNG</span><h1 id="providers-title">Provider wählen</h1><p>Lokaler CPU-Betrieb ist Standard. Ein eigener Remote-Worker erhält Audio nur nach bewusstem Test, Speichern und Aktivieren.</p></div></div>
        {#if !providerStatus || !remoteProviders}<div class="loading"><i></i> Komponenten und Providerprofile werden geprüft …</div>{:else}
          <section class="provider-status-card"><div class="provider-status-heading"><div><span class="pill pill-active">{providerStatus.backend.toUpperCase()}</span><h2>Lokaler whisper.cpp Worker</h2><p>{providerStatus.detail}</p></div><div class="button-row"><button class="button button-secondary" on:click={() => void refresh(false)}>Erneut prüfen</button><button class="button button-primary" disabled={busyAction === 'local-provider'} on:click={() => void useLocalProvider()}>Lokal verwenden</button></div></div><dl><div><dt>Worker-Sidecar</dt><dd class:ready={providerStatus.workerAvailable}>{providerStatus.workerAvailable ? 'bereit' : 'fehlt'}</dd></div><div><dt>CPU-Server</dt><dd class:ready={providerStatus.whisperServerAvailable}>{providerStatus.whisperServerAvailable ? 'bereit' : 'fehlt'}</dd></div><div><dt>Aktives Modell</dt><dd>{providerStatus.activeModelId ?? 'kein Modell aktiv'}</dd></div><div><dt>Validierung</dt><dd class:ready={providerStatus.activeModelValidated}>{providerStatus.activeModelValidated ? 'verifiziert' : 'ausstehend'}</dd></div></dl></section>

          <section class="remote-provider-card" aria-labelledby="remote-title"><div><span class="workspace-kicker">EIGENER RECHNER</span><h2 id="remote-title">Remote-Worker verbinden</h2><p>HTTPS ist Pflicht. Nur für einen expliziten Loopback-Entwicklertest kann HTTP aktiviert werden. Der Bearer-Token wird nach erfolgreichem Test ausschließlich im Windows Credential Manager abgelegt.</p></div><div class="remote-form"><label><span>Name</span><input bind:value={remoteDisplayName} maxlength="80" placeholder="z. B. Arbeitsrechner" /></label><label class="remote-wide"><span>HTTPS-Endpunkt</span><input bind:value={remoteEndpoint} inputmode="url" placeholder="https://worker.example.net" /></label><label class="remote-wide"><span>Bearer-Token</span><input type="password" bind:value={remoteBearerToken} autocomplete="off" placeholder="wird nie in SQLite oder Logs gespeichert" /></label><label><span>Connect-Timeout (ms)</span><input type="number" min="1000" max="60000" bind:value={remoteConnectTimeoutMs} /></label><label><span>Request-Timeout (ms)</span><input type="number" min="5000" max="600000" bind:value={remoteRequestTimeoutMs} /></label><label class="checkbox remote-wide"><input type="checkbox" bind:checked={remoteAllowHttpLoopback} /> Entwicklungsmodus: HTTP nur für 127.0.0.1/localhost erlauben</label><div class="button-row remote-wide"><button class="button button-secondary" disabled={busyAction === 'remote-test'} on:click={() => void testRemoteProvider()}>{busyAction === 'remote-test' ? 'Test läuft …' : 'Verbindung testen'}</button>{#if remoteTest}<button class="button button-primary" disabled={busyAction === 'remote-save' || !remoteTest.healthy || !remoteTest.remoteExecution || !remoteSelectedModelId} on:click={() => void saveRemoteProvider()}>{busyAction === 'remote-save' ? 'Wird sicher gespeichert …' : 'Speichern & aktivieren'}</button>{/if}</div></div>
            {#if remoteTest}<div class="remote-test-result"><div class="provider-status-heading"><div><span class:danger={!remoteTest.healthy || !remoteTest.remoteExecution} class="pill pill-active">{remoteTest.healthy && remoteTest.remoteExecution ? 'VERBINDUNG BEREIT' : 'NICHT KOMPATIBEL'}</span><p>{remoteTest.detail} · max. {formatBytes(remoteTest.maxRequestBytes)} · {formatDuration(remoteTest.maxAudioDurationMs)} Audio</p></div></div><label><span>Bereites Worker-Modell</span><select bind:value={remoteSelectedModelId}><option value="" disabled>Modell auswählen</option>{#each remoteTest.models as model}<option value={model.id} disabled={!model.installed || !model.ready}>{model.displayName} · {model.id}{model.ready ? '' : ' (nicht bereit)'}</option>{/each}</select></label><p class="muted">Spracherkennung: {remoteTest.languageDetection ? 'ja' : 'nein'} · Wortzeitstempel: {remoteTest.wordTimestamps ? 'ja' : 'nein'}. Ein Test speichert den Token noch nicht.</p></div>{/if}
          </section>

          {#if remoteProviders.profiles.length > 0}<section class="remote-profile-list" aria-label="Gespeicherte Remote-Provider"><h2>Gespeicherte eigene Rechner</h2>{#each remoteProviders.profiles as profile}<article class:active-profile={remoteProviders.activeProfileId === profile.id}><div><span class="pill">{remoteProviders.activeProfileId === profile.id ? 'Aktiv' : 'Remote'}</span><h3>{profile.displayName}</h3><p>{profile.endpoint} · {profile.selectedModelId ?? 'kein Modell'} · {profile.credentialConfigured ? 'Anmeldung im Credential Manager' : 'Anmeldung fehlt'}</p></div><div class="button-row"><button class="button button-secondary" disabled={remoteProviders.activeProfileId === profile.id || busyAction === `remote-activate-${profile.id}`} on:click={() => void activateRemoteProvider(profile.id)}>Aktivieren</button><button class="button-link danger" disabled={busyAction === `remote-delete-${profile.id}`} on:click={() => void deleteRemoteProvider(profile.id)}>Entfernen</button></div></article>{/each}</section>{/if}
        {/if}
      </section>
    {:else if page === 'settings'}
      <section class="page settings-page" aria-labelledby="settings-title">
        <div class="page-heading"><div><span class="workspace-kicker">WINDOWS-INTEGRATION</span><h1 id="settings-title">Sicher übergeben</h1><p>Copy bleibt der Standard. Automatisches Einfügen ist freiwillig und prüft den ursprünglichen Fensterkontext erneut.</p></div></div>
        <article class="settings-card"><div class="setting-title"><div><h2>Globaler Hotkey</h2><p>Schaltet die Aufnahme unabhängig vom aktiven Programm um.</p></div><kbd>{hotkeyLabel}</kbd></div><p class="muted">Die Tastenkombination wird beim App-Start registriert. Eine Kollision oder ungültige Bindung erscheint sichtbar statt still ignoriert zu werden.</p>{#if snapshot.hotkeyError}<p class="field-error">{snapshot.hotkeyError}</p>{/if}</article>
        <article class="settings-card"><div class="setting-title"><div><h2>Auto-Paste</h2><p>Nur in das beim Start gespeicherte, unveränderte und nicht erhöhte Fenster.</p></div><label class="toggle"><input type="checkbox" bind:checked={windowsSettings.autoPaste} /><span></span></label></div>{#if windowsSettings.autoPaste}<div class="safety-note"><b>Zusätzliche Prüfung aktiv.</b> Bei Fensterwechsel, UAC-Grenze oder Fokusfehler wird nie eingefügt; der Text bleibt nur in der Zwischenablage.</div>{/if}</article>
        <article class="settings-card"><div class="setting-title"><div><h2>Zwischenablage nach Einfügen</h2><p>Nur ursprünglichen Klartext wiederherstellen – und nur, wenn Windows keinen Zwischenzeitwechsel meldet.</p></div><label class="toggle"><input type="checkbox" bind:checked={windowsSettings.restoreClipboardAfterPaste} disabled={!windowsSettings.autoPaste} /><span></span></label></div><p class="muted">Nach 400 ms prüft die Anwendung den Windows-Sequence-Counter. Dateien, Bilder und unbekannte Formate werden nicht geraten oder überschrieben.</p></article>
        <button class="button button-primary" disabled={busyAction === 'windows-settings'} on:click={saveWindowsSettings}>{busyAction === 'windows-settings' ? 'Wird gespeichert …' : 'Windows-Einstellungen speichern'}</button>
      </section>
    {:else}
      <section class="page home-page" aria-labelledby="home-title">
        <div class="page-heading home-heading"><div><span class="workspace-kicker">AUFNAHME</span><h1 id="home-title">Dein Gespräch.<br />Dein Text.</h1><p>{snapshot.activeProvider?.kind === 'remote' ? 'Du verwendest bewusst deinen eigenen Remote-Worker. Audio wird ausschließlich an den unter Provider geprüften Endpunkt übertragen.' : 'Aufnahme, Verarbeitung und Verlauf bleiben auf diesem Rechner. Das Modell läuft über einen isolierten lokalen Worker.'}</p></div><div class="model-summary"><span>AKTIVER PROVIDER</span><b>{snapshot.activeProvider?.displayName}</b><small>{snapshot.activeProvider?.backend.toUpperCase()} · {snapshot.activeProvider?.modelId}</small></div></div>
        <div class="recording-layout">
          <section class="record-panel" aria-label="Aufnahme"><div class="record-panel-head"><span class="presence"><span class:recording={currentRecording !== null}></span>{currentRecording ? 'Aufnahme läuft' : snapshot.processing ? 'Transkription läuft' : 'Bereit'}</span><span>{currentRecording ? formatDuration(currentRecording.durationMs) : '0:00'}</span></div><div class="wave" class:wave-live={currentRecording !== null} style={`--level: ${(currentRecording?.peakMilli ?? 0) / 1000}`} aria-label={`Mikrofonpegel ${currentRecording?.peakMilli ?? 0} von 1000`}>{#each Array(22) as _, index}<i style={`--i: ${index}`}></i>{/each}</div><p>{currentRecording ? 'Audio bleibt im begrenzten Arbeitsspeicher.' : 'Starte eine lokale Aufnahme, wenn du bereit bist.'}</p>{#if currentRecording}<button class="record-action stop" disabled={busyAction !== null} on:click={finishRecording}>■ &nbsp; Stop & transkribieren</button><button class="button-link center" on:click={cancelCurrent}>Aufnahme verwerfen</button>{:else}<button class="record-action" disabled={isWorking} on:click={() => settings.mode === 'toggle' && void beginRecording()} on:pointerdown={(event) => { if (settings.mode === 'push_to_talk') { event.preventDefault(); void beginRecording(); } }} on:pointerup={() => settings.mode === 'push_to_talk' && currentRecording && void finishRecording()} on:pointercancel={() => settings.mode === 'push_to_talk' && currentRecording && void cancelCurrent()}>● &nbsp; {settings.mode === 'push_to_talk' ? 'Gedrückt halten' : 'Aufnahme starten'}</button>{/if}</section>
          <section class="capture-settings" aria-labelledby="capture-settings-title"><h2 id="capture-settings-title">Aufnahmeprofil</h2><label><span>Mikrofon</span><select bind:value={settings.deviceId} disabled={currentRecording !== null}><option value={null}>Systemstandard</option>{#each snapshot.inputDevices as device}<option value={device.id}>{device.name} · {device.sampleRateHz / 1000} kHz</option>{/each}</select></label>{#if snapshot.microphoneError}<p class="field-error">{snapshot.microphoneError}</p>{/if}<fieldset disabled={currentRecording !== null}><legend>Modus</legend><label><input type="radio" bind:group={settings.mode} value="toggle" /> Umschalten</label><label><input type="radio" bind:group={settings.mode} value="push_to_talk" /> Push-to-talk</label></fieldset><label><span>Maximale Aufnahmezeit</span><input type="number" min="15" max="600" step="1" bind:value={settings.maxDurationSeconds} disabled={currentRecording !== null} /><small>15–600 Sekunden, Standard 180 Sekunden.</small></label><label class="checkbox"><input type="checkbox" bind:checked={settings.vad.enabled} /> Stille per VAD erkennen</label><p class="muted">30 ms Frames · Aggressivität {settings.vad.aggressiveness} · mindestens {settings.vad.minimumSpeechMs} ms Sprache.</p></section>
        </div>
        {#if result}<section class="result-panel" aria-labelledby="result-title"><div class="result-heading"><div><span class="workspace-kicker">LETZTES ERGEBNIS</span><h2 id="result-title">Bereit zur Übergabe</h2></div><span class="pill">{outcomeLabel()}</span></div><p class="result-text">{result.finalText}</p><div class="button-row"><button class="button button-primary" disabled={busyAction === 'copy'} on:click={copyResult}>{busyAction === 'copy' ? 'Kopiert …' : 'Text kopieren'}</button>{#if result.canPasteToOriginal && result.injectionOutcome !== 'inserted'}<button class="button button-secondary" disabled={busyAction === 'paste'} on:click={pasteOriginal}>{busyAction === 'paste' ? 'Prüft Ziel …' : 'Im Originalfenster einfügen'}</button>{/if}<button class="button-link" on:click={() => (result = null)}>Schließen</button></div>{#if result.corrections.length > 0}<div class="corrections"><b>Nachkorrekturen</b>{#each result.corrections as correction}<span class:reverted={correction.reverted}><s>{correction.original}</s> → {correction.replacement}{#if correction.reverted}<small>zurückgenommen</small>{:else}<button class="button-link" disabled={busyAction === 'correction-' + correction.id} on:click={() => void revertCorrection(result?.id ?? '', correction.id)}>Rückgängig</button>{/if}</span>{/each}</div>{/if}<details><summary>Rohtext, Laufzeiten und Warnungen</summary><p class="raw-text">{result.rawText}</p><div class="timings"><span>Audio {formatDuration(result.audioDurationMs)}</span><span>Laden {formatDuration(result.modelLoadMs)}</span><span>Inferenz {formatDuration(result.inferenceMs)}</span><span>Korrektur {formatDuration(result.correctionMs)}</span></div>{#if result.warnings.length > 0}<ul>{#each result.warnings as warning}<li>{warning}</li>{/each}</ul>{/if}</details></section>{/if}
      </section>
    {/if}
  </section>
</main>
{/if}
