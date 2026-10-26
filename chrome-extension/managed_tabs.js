(() => {
  const MANAGED_TAB_IDENTITY_POLICY_VERSION = 1;
  let managedTabs = new Map();
  let managedTabOwners = new Map();
  const pendingStartupIdentityChecks = new Map();
  const closingManagedTabIds = new Set();
  let managedTabsSaveQueue = Promise.resolve();
  let managedTabOwnersSaveQueue = Promise.resolve();
  let workerCallbacks = null;
  let closeOnRaidRedirect = false;
  let raidRedirectedChannelIds = new Set();
  let raidRedirectedChannelIdsSaveQueue = Promise.resolve();
  let browserStartupOwnershipInvalidated = false;
  let adoptExistingTwitchTabs = false;
  const pendingAdoptions = new Set();

function hasRaidReferrer(value) {
  try {
    return new URL(value).searchParams.getAll("referrer").includes("raid");
  } catch {
    return false;
  }
}

function urlMatchesManagedSurface(value, login, surface, layout) {
  const route = TwitchTabUtils.channelRoute(value);
  if (route.login !== login) return false;
  if (layout === "combined_window" || layout === "regular_website") {
    return route.website && ["player", "chat"].includes(surface);
  }
  return !route.website && route.surface === surface;
}

function matchesManagedRecordRoute(record, value, surface) {
  const logins = [
    record.login,
    ...(Array.isArray(record.previousLogins) ? record.previousLogins : []),
  ];
  return logins.some((login) => (
    urlMatchesManagedSurface(value, login, surface, record.layout)
  ));
}
function bindWorkerCallbacks(callbacks) {
  workerCallbacks = callbacks;
}
function snapshotRecord(record) {
  if (!record) return record;
  const snapshot = { ...record };
  if (Array.isArray(record.previousLogins)) {
    snapshot.previousLogins = Object.freeze([...record.previousLogins]);
  }
  return Object.freeze(snapshot);
}
function isCloseOnRaidRedirectEnabled() {
  return closeOnRaidRedirect;
}

async function setCloseOnRaidRedirect(enabled) {
  closeOnRaidRedirect = enabled === true;
  return chrome.storage.local.set({ closeOnRaidRedirect });
}

function getRaidRedirectedChannelIds() {
  return Array.from(raidRedirectedChannelIds).sort();
}

function saveRaidRedirectedChannelIds() {
  const saving = raidRedirectedChannelIdsSaveQueue.then(() =>
    chrome.storage.local.set({
      raidRedirectedChannelIds: getRaidRedirectedChannelIds(),
    }),
  );
  raidRedirectedChannelIdsSaveQueue = saving.catch(() => {});
  return saving;
}

async function clearRaidRedirectedChannelIds(channelIds) {
  const removedChannelIds = [];
  for (const channelId of new Set(channelIds.filter((id) => typeof id === "string"))) {
    if (raidRedirectedChannelIds.delete(channelId)) removedChannelIds.push(channelId);
  }
  if (removedChannelIds.length === 0) return;
  try {
    await saveRaidRedirectedChannelIds();
  } catch (error) {
    for (const channelId of removedChannelIds) raidRedirectedChannelIds.add(channelId);
    console.warn("Could not persist cleared raid redirect channels.", error);
    return;
  }
  workerCallbacks.scheduleExternalStreamReport();
}

async function markRaidRedirectedChannel(channelId) {
  raidRedirectedChannelIds.add(channelId);
  try {
    await saveRaidRedirectedChannelIds();
  } catch (error) {
    console.warn("Could not persist a raid-redirected channel.", error);
  }
}

function matchesPendingRoute(record, value, surface) {
  return urlMatchesManagedSurface(value, record.login, surface, record.layout);
}

async function restoreManagedTabs() {
  const stored = await chrome.storage.local.get([
    "managedTabs",
    "closeOnRaidRedirect",
    "raidRedirectedChannelIds",
    "managedTabOwners",
    "adoptExistingTwitchTabs",
  ]);
  adoptExistingTwitchTabs = stored.adoptExistingTwitchTabs === true;
  managedTabs = new Map(stored.managedTabs || []);
  closeOnRaidRedirect = stored.closeOnRaidRedirect === true;
  raidRedirectedChannelIds = new Set(
    Array.isArray(stored.raidRedirectedChannelIds)
      ? stored.raidRedirectedChannelIds.filter((channelId) => typeof channelId === "string")
      : [],
  );
  managedTabOwners = browserStartupOwnershipInvalidated
    ? new Map()
    : new Map(stored.managedTabOwners || []);
  if (browserStartupOwnershipInvalidated) {
    for (const record of managedTabs.values()) delete record.playerWindowId;
  }
  const tabs = await chrome.tabs.query({});
  const tabsById = new Map(tabs.map((tab) => [tab.id, tab]));
  const raidRedirectedChannels = new Set();
  const claimedTabIds = new Set();
  for (const [channelId, record] of managedTabs) {
    if (record.tabId && !record.playerTabId) {
      record.playerTabId = record.tabId;
      delete record.tabId;
    }
    for (const surface of ["player", "chat"]) {
      const key = `${surface}TabId`;
      const tab = tabsById.get(record[key]);
      if (record.adoptedExternal === true) {
        const route = TwitchTabUtils.channelRoute(tab?.url || "");
        const identity = tab ? await readManagedTabIdentity(tab.id) : null;
        if (
          !adoptExistingTwitchTabs
          || surface !== "player"
          || !tab
          || Number.isInteger(tab.openerTabId)
          || record.identityPersisted !== true
          || record.identityPolicyVersion !== MANAGED_TAB_IDENTITY_POLICY_VERSION
          || typeof record.identityToken !== "string"
          || identity !== record.identityToken
          || route.surface !== "player"
          || route.login !== record.login.toLowerCase()
        ) {
          record[key] = null;
          pendingStartupIdentityChecks.delete(tab?.id);
          if (tab) managedTabOwners.delete(tab.id);
          if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
          continue;
        }
      }
      const owner = tab && managedTabOwners.get(tab.id);
      const raidPlayerRoute = surface === "player"
        && closeOnRaidRedirect
        && hasRaidReferrer(tab?.url || "")
        && TwitchTabUtils.channelRoute(tab?.url || "").surface === "player";
      if (
        surface === "player"
        && record.pendingRaidOwnershipVerification === true
        && !raidPlayerRoute
      ) delete record.pendingRaidOwnershipVerification;
      const tabIdentity = raidPlayerRoute
        ? await readManagedTabIdentity(tab.id)
        : null;
      const identityUnsafe = Number.isInteger(tab?.openerTabId);
      const identityMatches = !identityUnsafe
        && record.identityPolicyVersion === MANAGED_TAB_IDENTITY_POLICY_VERSION
        && typeof record.identityToken === "string"
        && tabIdentity === record.identityToken;
      const identityUnavailable = tabIdentity === undefined
        || tabIdentity === null
        || identityUnsafe;
      if (
        raidPlayerRoute
        && (
          owner === channelId
          || (
            !browserStartupOwnershipInvalidated
            && Number.isInteger(record.playerWindowId)
            && record.playerWindowId === tab.windowId
          )
          || identityMatches
        )
      ) {
        raidRedirectedChannels.add(channelId);
        claimedTabIds.add(tab.id);
        continue;
      }
      if (
        raidPlayerRoute
        && (
          record.pendingRaidOwnershipVerification === true
          || identityUnsafe
          || (
            browserStartupOwnershipInvalidated
            && (
              typeof record.identityToken !== "string"
              || record.identityPolicyVersion !== MANAGED_TAB_IDENTITY_POLICY_VERSION
              || (record.identityPersisted !== true && identityUnavailable)
            )
          )
          || (record.identityPersisted === false && identityUnavailable)
        )
      ) {
        record.pendingRaidOwnershipVerification = true;
        pendingStartupIdentityChecks.set(tab.id, channelId);
        claimedTabIds.add(tab.id);
        continue;
      }
      if (
        raidPlayerRoute
        && typeof record.identityToken === "string"
        && tabIdentity === undefined
      ) {
        pendingStartupIdentityChecks.set(tab.id, channelId);
        claimedTabIds.add(tab.id);
        continue;
      }
      const ownershipMatches = tab && (
        owner === channelId
        || (owner === undefined && (
          surface !== "player"
          || !Number.isInteger(record.playerWindowId)
          || record.playerWindowId === tab.windowId
        ))
      );
      const currentPage = ownershipMatches && matchesManagedRecordRoute(record, tab.url, surface);
      const pendingPage = ownershipMatches
        && tab.url === "about:blank"
        && matchesPendingRoute(record, record.pendingUrl, surface);
      if ((currentPage || pendingPage) && tab) {
        claimedTabIds.add(tab.id);
        managedTabOwners.set(tab.id, channelId);
        if (surface === "player") record.playerWindowId = tab.windowId;
      } else {
        if (
          tab
          && owner === channelId
          && !raidRedirectedChannels.has(channelId)
        ) managedTabOwners.delete(tab.id);
        if (surface === "player") delete record.playerWindowId;
        record[key] = null;
      }
    }
    if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  }
  for (const tabId of managedTabOwners.keys()) {
    if (!claimedTabIds.has(tabId)) managedTabOwners.delete(tabId);
  }
  try {
    await saveManagedTabOwners();
  } catch (error) {
    console.warn("Could not persist restored managed tab ownership.", error);
  }
  await saveManagedTabs();
  if (raidRedirectedChannels.size > 0) {
    await workerCallbacks.closeChannels(Array.from(raidRedirectedChannels), "raid_redirect");
  }
}
async function readManagedTabIdentity(tabId) {
  try {
    const result = await chrome.tabs.sendMessage(tabId, {
      type: "get_managed_tab_identity",
    });
    return typeof result?.identity === "string" ? result.identity : null;
  } catch {
    return undefined;
  }
}

async function resolvePendingStartupIdentity(tabId, identity) {
  const channelId = pendingStartupIdentityChecks.get(tabId);
  if (!channelId) return false;
  pendingStartupIdentityChecks.delete(tabId);
  const record = managedTabs.get(channelId);
  if (!record || record.playerTabId !== tabId) return false;
  let tab;
  try {
    tab = await chrome.tabs.get(tabId);
  } catch {
    record.playerTabId = null;
    if (record.chatTabId === tabId) record.chatTabId = null;
    delete record.pendingRaidOwnershipVerification;
    if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
    await saveManagedTabs();
    workerCallbacks.scheduleExternalStreamReport();
    return false;
  }
  const route = TwitchTabUtils.channelRoute(tab.url || "");
  const identityUnsafe = Number.isInteger(tab.openerTabId);
  const identityUnavailable = identity === undefined || identity === null || identityUnsafe;
  if (
    record.pendingRaidOwnershipVerification === true
    && (
      typeof record.identityToken !== "string"
      || record.identityPolicyVersion !== MANAGED_TAB_IDENTITY_POLICY_VERSION
      || identityUnsafe
      || (record.identityPersisted !== true && identityUnavailable)
    )
    && route.surface === "player"
    && hasRaidReferrer(tab.url || "")
  ) {
    pendingStartupIdentityChecks.set(tabId, channelId);
    return false;
  }
  delete record.pendingRaidOwnershipVerification;
  if (
    !identityUnsafe
    && record.identityPolicyVersion === MANAGED_TAB_IDENTITY_POLICY_VERSION
    && identity === record.identityToken
  ) {
    if (
      closeOnRaidRedirect
      && route.surface === "player"
      && hasRaidReferrer(tab.url || "")
    ) {
      await workerCallbacks.closeChannels([channelId], "raid_redirect");
      return true;
    }
    if (matchesManagedRecordRoute(record, tab.url, "player")) {
      managedTabOwners.set(tabId, channelId);
      record.playerWindowId = tab.windowId;
      await saveManagedTabOwners();
      await saveManagedTabs();
      return true;
    }
  }
  record.playerTabId = null;
  if (record.chatTabId === tabId) record.chatTabId = null;
  delete record.pendingRaidOwnershipVerification;
  managedTabOwners.delete(tabId);
  if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  await saveManagedTabOwners();
  await saveManagedTabs();
  workerCallbacks.scheduleExternalStreamReport();
  return false;
}

async function invalidateManagedTabOwners() {
  browserStartupOwnershipInvalidated = true;
  managedTabOwners = new Map();
  try {
    const stored = await chrome.storage.local.get("managedTabs");
    const records = new Map(stored.managedTabs || []);
    for (const record of records.values()) delete record.playerWindowId;
    await chrome.storage.local.set({
      managedTabs: Array.from(records.entries()),
      managedTabOwners: [],
    });
  } catch (error) {
    console.warn("Could not clear managed ownership from the prior browser session.", error);
  }
}

function saveManagedTabs() {
  const snapshot = Array.from(managedTabs.entries());
  const saving = managedTabsSaveQueue.then(() =>
    chrome.storage.local.set({ managedTabs: snapshot }),
  );
  managedTabsSaveQueue = saving.catch(() => {});
  return saving;
}

function saveManagedTabOwners() {
  const saving = managedTabOwnersSaveQueue.then(() =>
    chrome.storage.local.set({
      managedTabOwners: Array.from(managedTabOwners.entries()),
    }),
  );
  managedTabOwnersSaveQueue = saving.catch(() => {});
  return saving;
}
function saveManagedTabState() {
  const saving = Promise.all([managedTabsSaveQueue, managedTabOwnersSaveQueue]).then(() =>
    chrome.storage.local.set({
      managedTabs: Array.from(managedTabs.entries()),
      managedTabOwners: Array.from(managedTabOwners.entries()),
      adoptExistingTwitchTabs,
    }),
  );
  managedTabsSaveQueue = saving.catch(() => {});
  managedTabOwnersSaveQueue = saving.catch(() => {});
  return saving;
}

async function rememberManagedTabOwner(tabId, channelId) {
  if (managedTabOwners.get(tabId) === channelId) return;
  managedTabOwners.set(tabId, channelId);
  try {
    await saveManagedTabOwners();
  } catch (error) {
    console.warn("Could not persist managed tab ownership.", error);
  }
}

async function forgetManagedTabOwner(tabId) {
  if (!managedTabOwners.delete(tabId)) return;
  try {
    await saveManagedTabOwners();
  } catch (error) {
    console.warn("Could not clear managed tab ownership.", error);
  }
}
async function setPlayerAudioPreference(tabId, unmuted, login) {
  try {
    await chrome.tabs.sendMessage(tabId, {
      type: "set_player_audio_preference",
      login,
      unmuted,
    });
  } catch {}
}

async function applyManagedPlayerAudioPreference(tabId, reportedIdentity) {
  const tab = await chrome.tabs.get(tabId).catch(() => null);
  if (!tab || Number.isInteger(tab.openerTabId)) return false;
  const route = TwitchTabUtils.channelRoute(tab.url || "");
  if (route.surface !== "player" || !route.login) return false;
  for (const [channelId, record] of managedTabs) {
    if (record.playerTabId !== tabId) continue;
    if (
      record.identityPersisted !== true
      || managedTabOwners.get(tabId) !== channelId
      || route.login !== record.login.toLowerCase()
    ) return false;
    const identity = reportedIdentity === undefined
      ? await readManagedTabIdentity(tabId)
      : reportedIdentity;
    if (identity !== record.identityToken) return false;
    if (typeof record.unmuted === "boolean") {
      await setPlayerAudioPreference(tabId, record.unmuted, route.login);
    }
    return true;
  }
  return false;
}
async function setManagedTabIdentity(tabId, identity) {
  try {
    const tab = await chrome.tabs.get(tabId);
    if (Number.isInteger(tab.openerTabId)) return false;
    const result = await chrome.tabs.sendMessage(tabId, {
      type: "set_managed_tab_identity",
      identity,
    });
    return result?.identity === identity;
  } catch {
    return false;
  }
}
async function setExistingTabAdoption(enabled) {
  const previousAdoption = adoptExistingTwitchTabs;
  const releasedRecords = [];
  adoptExistingTwitchTabs = enabled === true;
  if (!adoptExistingTwitchTabs) {
    for (const [channelId, record] of managedTabs) {
      if (record.adoptedExternal !== true) continue;
      const tabId = record.playerTabId;
      releasedRecords.push({
        channelId,
        record,
        tabId,
        owner: managedTabOwners.get(tabId),
        pendingIdentityCheck: pendingStartupIdentityChecks.get(tabId),
      });
      managedTabs.delete(channelId);
      pendingStartupIdentityChecks.delete(tabId);
      managedTabOwners.delete(tabId);
    }
  }
  try {
    await saveManagedTabState();
  } catch (error) {
    adoptExistingTwitchTabs = previousAdoption;
    const staleReleasedTabs = [];
    for (const {
      channelId,
      record,
      tabId,
      owner,
      pendingIdentityCheck,
    } of releasedRecords) {
      const currentRecord = managedTabs.get(channelId);
      const currentOwner = managedTabOwners.get(tabId);
      if (
        (currentRecord && currentRecord !== record)
        || (currentOwner !== undefined && currentOwner !== owner)
      ) {
        if (Number.isInteger(tabId)) staleReleasedTabs.push({ channelId, tabId });
        continue;
      }
      managedTabs.set(channelId, record);
      if (owner !== undefined) managedTabOwners.set(tabId, owner);
      if (!await verifyAdoptedTabOwnership(channelId, tabId)) {
        if (managedTabs.get(channelId) === record) managedTabs.delete(channelId);
        if (managedTabOwners.get(tabId) === owner) managedTabOwners.delete(tabId);
        if (Number.isInteger(tabId)) staleReleasedTabs.push({ channelId, tabId });
        continue;
      }
      if (
        pendingIdentityCheck !== undefined
        && !pendingStartupIdentityChecks.has(tabId)
      ) {
        pendingStartupIdentityChecks.set(tabId, pendingIdentityCheck);
      }
    }
    if (staleReleasedTabs.length > 0) {
      const rollbackError = error instanceof Error ? error : new Error(String(error));
      rollbackError.releasedTabs = staleReleasedTabs;
      await saveManagedTabState().catch(() => {});
      throw rollbackError;
    }
    throw error;
  }
  return releasedRecords
    .filter(({ tabId }) => Number.isInteger(tabId))
    .map(({ channelId, tabId }) => ({ channelId, tabId }));
}

async function adoptExistingPlayerTab(channel, candidate, isStillEligible) {
  const channelId = channel?.id;
  const login = channel?.login?.toLowerCase();
  if (
    !adoptExistingTwitchTabs
    || typeof channelId !== "string"
    || !channelId
    || typeof login !== "string"
    || !Number.isInteger(candidate?.id)
    || !Number.isInteger(candidate?.windowId)
    || candidate.status !== "complete"
    || Number.isInteger(candidate.openerTabId)
    || pendingAdoptions.has(channelId)
    || managedTabs.has(channelId)
    || managedTabOwners.has(candidate.id)
      || typeof isStillEligible !== "function"
      || !isStillEligible()
  ) return false;
  const initialRoute = TwitchTabUtils.channelRoute(candidate.url || "");
  if (initialRoute.surface !== "player" || initialRoute.login !== login) return false;
  pendingAdoptions.add(channelId);
  try {
    if (!adoptExistingTwitchTabs || managedTabs.has(channelId) || !isStillEligible()) return false;
    const current = await chrome.tabs.get(candidate.id).catch(() => null);
    if (!isStillEligible()) return false;
    if (
      !current
      || current.status !== "complete"
      || Number.isInteger(current.openerTabId)
      || TwitchTabUtils.channelRoute(current.url || "").login !== login
      || TwitchTabUtils.channelRoute(current.url || "").surface !== "player"
    ) return false;
    const identityToken = crypto.randomUUID();
    const identityWasSet = await setManagedTabIdentity(candidate.id, identityToken);
    if (!isStillEligible() || !identityWasSet) return false;
    const verifiedTab = await chrome.tabs.get(candidate.id).catch(() => null);
    if (!isStillEligible()) return false;
    if (
      !adoptExistingTwitchTabs
      || managedTabs.has(channelId)
      || managedTabOwners.has(candidate.id)
      || !verifiedTab
      || Number.isInteger(verifiedTab.openerTabId)
      || TwitchTabUtils.channelRoute(verifiedTab.url || "").login !== login
      || TwitchTabUtils.channelRoute(verifiedTab.url || "").surface !== "player"
    ) return false;
    const identity = await readManagedTabIdentity(candidate.id);
    if (
      !isStillEligible()
      || identity !== identityToken
      || !adoptExistingTwitchTabs
      || managedTabs.has(channelId)
      || managedTabOwners.has(candidate.id)
    ) return false;
    const record = {
      login,
      playerTabId: candidate.id,
      chatTabId: null,
      playerWindowId: verifiedTab.windowId,
      layout: "combined_window",
      unmuted: channel.unmuted === true,
      identityToken,
      identityPersisted: true,
      identityPolicyVersion: MANAGED_TAB_IDENTITY_POLICY_VERSION,
      adoptedExternal: true,
    };
    managedTabs.set(channelId, record);
    managedTabOwners.set(candidate.id, channelId);
    try {
      await saveManagedTabState();
    } catch (error) {
      if (managedTabs.get(channelId) === record) managedTabs.delete(channelId);
      if (managedTabOwners.get(candidate.id) === channelId) {
        managedTabOwners.delete(candidate.id);
      }
      await saveManagedTabState().catch(() => {});
      throw error;
    }
    const ownershipVerified =
      adoptExistingTwitchTabs
      && isStillEligible()
      && await verifyAdoptedTabOwnership(channelId, candidate.id)
      && isStillEligible();
    if (!ownershipVerified) {
      if (managedTabs.get(channelId) === record) managedTabs.delete(channelId);
      if (managedTabOwners.get(candidate.id) === channelId) {
        managedTabOwners.delete(candidate.id);
      }
      await saveManagedTabState().catch(() => {});
      return false;
    }
    return true;
  } finally {
    pendingAdoptions.delete(channelId);
  }
}

async function verifyAdoptedTabOwnership(channelId, tabId, allowRaidRedirect = false) {
  const record = managedTabs.get(channelId);
  if (
    !record
    || record.adoptedExternal !== true
    || record.playerTabId !== tabId
    || managedTabOwners.get(tabId) !== channelId
    || record.identityPersisted !== true
    || typeof record.identityToken !== "string"
  ) return false;
  const tab = await chrome.tabs.get(tabId).catch(() => null);
  if (!tab || Number.isInteger(tab.openerTabId)) return false;
  const identity = await readManagedTabIdentity(tabId);
  if (
    identity !== record.identityToken
    || managedTabs.get(channelId) !== record
    || managedTabOwners.get(tabId) !== channelId
  ) return false;
  const latestTab = await chrome.tabs.get(tabId).catch(() => null);
  if (
    !latestTab
    || Number.isInteger(latestTab.openerTabId)
    || managedTabs.get(channelId) !== record
    || managedTabOwners.get(tabId) !== channelId
  ) return false;
  const route = TwitchTabUtils.channelRoute(latestTab.url || "");
  const raidRedirectAllowed = allowRaidRedirect
    && route.surface === "player"
    && hasRaidReferrer(latestTab.url || "");
  return matchesManagedRecordRoute(record, latestTab.url, "player") || raidRedirectAllowed;
}

function managedSurfaceTabs(record) {
  return [
    ["player", record.playerTabId],
    ["chat", record.chatTabId],
  ].filter(([, tabId]) => Number.isInteger(tabId));
}
async function setPlayerWindow(channelId, tabId, windowId) {
  const record = managedTabs.get(channelId);
  if (!record || record.playerTabId !== tabId) return false;
  record.playerWindowId = windowId;
  await saveManagedTabs();
  return true;
}

function removeClosedManagedSurfaces(channelId, tabId, surfaces) {
  const record = managedTabs.get(channelId);
  if (!record) return [];
  const removedSurfaces = [];
  for (const surface of surfaces) {
    const key = `${surface}TabId`;
    if (record[key] !== tabId) continue;
    record[key] = null;
    if (surface === "player") delete record.pendingRaidOwnershipVerification;
    removedSurfaces.push(surface);
  }
  if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  return removedSurfaces;
}

async function detachManagedTab(channelId, tabId) {
  const record = managedTabs.get(channelId);
  if (!record) return [];
  const detachedSurfaces = managedSurfaceTabs(record)
    .filter(([, managedTabId]) => managedTabId === tabId)
    .map(([surface]) => surface);
  if (detachedSurfaces.length === 0) return [];
  for (const surface of detachedSurfaces) {
    record[`${surface}TabId`] = null;
    if (surface === "player") {
      delete record.pendingRaidOwnershipVerification;
      pendingStartupIdentityChecks.delete(tabId);
    }
  }
  workerCallbacks.setMonitoring(tabId, "stop_monitoring");
  if (record.playerTabId !== tabId && record.chatTabId !== tabId) {
    await forgetManagedTabOwner(tabId);
  }
  if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  try {
    await saveManagedTabs();
  } catch (error) {
    console.warn("Could not persist detached managed tab state.", error);
  }
  return detachedSurfaces;
}
async function ensureDedicatedTabWindow(tabId) {
  const tab = await chrome.tabs.get(tabId);
  const window = await chrome.windows.get(tab.windowId, { populate: true });
  const windowTabs = window.tabs || await chrome.tabs.query({ windowId: tab.windowId });
  if (windowTabs.length > 1) {
    await chrome.windows.create({ tabId, focused: false, type: "normal" });
  }
}

async function createManagedTab() {
  const window = await chrome.windows.create({
    url: "about:blank",
    focused: false,
    type: "normal",
  });
  if (!Number.isInteger(window?.id)) {
    throw new Error("Could not create a dedicated Twitch window.");
  }
  const [tab] = await chrome.tabs.query({ windowId: window.id });
  if (!Number.isInteger(tab?.id)) {
    throw new Error("Could not create a Twitch tab in its window.");
  }
  return tab;
}

async function activateManagedTab(tabId, wakeBrowser, changes = {}) {
  const tab = await chrome.tabs.get(tabId);
  if (wakeBrowser) {
    await chrome.windows.update(tab.windowId, { state: "normal", focused: true });
  }
  await chrome.tabs.update(tabId, { ...changes, active: true });
}

function waitForTabLoad(tabId) {
  let finish;
  const promise = new Promise((resolve) => {
    let timeout;
    const settle = (loaded) => {
      chrome.tabs.onUpdated.removeListener(onUpdated);
      clearTimeout(timeout);
      resolve(loaded);
    };
    const onUpdated = (updatedTabId, changeInfo) => {
      if (updatedTabId === tabId && changeInfo.status === "complete") settle(true);
    };
    finish = settle;
    chrome.tabs.onUpdated.addListener(onUpdated);
    timeout = setTimeout(() => settle(false), 30000);
  });
  return { promise, cancel: (loaded) => finish(loaded) };
}

async function loadManagedTab(tabId, url, wakeBrowser, forceReload) {
  const tab = await chrome.tabs.get(tabId);
  if (!forceReload && !tab.discarded && tab.url === url && tab.status === "complete") {
    await activateManagedTab(tabId, wakeBrowser);
    return;
  }
  const loading = waitForTabLoad(tabId);
  try {
    const changes = {};
    if (forceReload || tab.url !== url || (!tab.discarded && tab.status !== "loading")) {
      changes.url = url;
    }
    await activateManagedTab(tabId, wakeBrowser, changes);
    const latest = await chrome.tabs.get(tabId);
    if (latest.url === url && latest.status === "complete" && !latest.discarded) {
      loading.cancel(true);
    }
    if (!await loading.promise) {
      throw new Error("Twitch page did not finish loading within 30 seconds.");
    }
  } catch (error) {
    loading.cancel(false);
    throw error;
  }
}

async function prepareChannelOpen(channel) {
  const login = channel.login.toLowerCase();
  let record = managedTabs.get(channel.id);
  if (
    record
    && (
      record.pendingRaidOwnershipVerification === true
      || pendingStartupIdentityChecks.get(record.playerTabId) === channel.id
    )
  ) return false;
  if (
    record?.layout === "regular_website"
    && Number.isInteger(record.playerTabId)
    && record.playerTabId === record.chatTabId
  ) {
    record.layout = "combined_window";
    await saveManagedTabs();
  }
  if (
    record
    && (record.layout !== "combined_window" || record.chatTabId !== record.playerTabId)
  ) {
    await workerCallbacks.closeChannels([channel.id]);
    record = managedTabs.get(channel.id) || null;
    if (record) return false;
  }
  record = record || {
    login,
    playerTabId: null,
    chatTabId: null,
  };
  if (record.login !== login) {
    const previousLogins = new Set(
      Array.isArray(record.previousLogins) ? record.previousLogins : [],
    );
    previousLogins.add(record.login);
    record.previousLogins = Array.from(previousLogins);
  }
  record.login = login;
  record.layout = "combined_window";
  managedTabs.set(channel.id, record);
  return true;
}

async function finishChannelOpen(channelId) {
  const record = managedTabs.get(channelId);
  if (!record) return null;
  delete record.previousLogins;
  record.chatTabId = record.playerTabId;
  await saveManagedTabs();
  return record.playerTabId;
}
async function openManagedTab(channelId, url, wakeBrowser, unmuted) {
  const record = managedTabs.get(channelId);
  if (!record) throw new Error("Managed channel record was not prepared for opening.");
  const audioPreferenceChanged = record.unmuted !== unmuted;
  record.unmuted = unmuted;
  let tabId = record.playerTabId;
  let isNewTab = tabId === null || tabId === undefined;
  record.identityToken = record.identityToken || crypto.randomUUID();
  if (tabId !== null && tabId !== undefined) {
    try {
      await chrome.tabs.get(tabId);
    } catch {
      record.playerTabId = null;
      record.chatTabId = null;
      tabId = null;
      isNewTab = true;
    }
  }
  if (tabId === null || tabId === undefined) {
    const tab = await createManagedTab();
    tabId = tab.id;
    record.playerTabId = tabId;
    record.chatTabId = tabId;
    record.pendingUrl = url;
    managedTabs.set(channelId, record);
  } else {
    await ensureDedicatedTabWindow(tabId);
  }
  const tabChanges = { autoDiscardable: false };
  if (isNewTab || audioPreferenceChanged) tabChanges.muted = !unmuted;
  await chrome.tabs.update(tabId, tabChanges);
  if (isNewTab) await saveManagedTabs();
  await rememberManagedTabOwner(tabId, channelId);
  const tab = await chrome.tabs.get(tabId);
  record.playerWindowId = tab.windowId;
  managedTabs.set(channelId, record);
  await saveManagedTabs();
  const contentVersion = chrome.runtime.getManifest().version;
  const forceReload = record.contentVersion !== contentVersion;
  if (tab.url !== url || tab.discarded || tab.status !== "complete" || forceReload) {
    record.pendingUrl = url;
    managedTabs.set(channelId, record);
    await saveManagedTabs();
  }
  await loadManagedTab(tabId, url, wakeBrowser, forceReload);
  record.identityPersisted = await setManagedTabIdentity(tabId, record.identityToken);
  if (record.identityPersisted) {
    record.identityPolicyVersion = MANAGED_TAB_IDENTITY_POLICY_VERSION;
  } else {
    delete record.identityPolicyVersion;
  }
  delete record.pendingUrl;
  record.contentVersion = contentVersion;
  managedTabs.set(channelId, record);
  await saveManagedTabs();
  if (isNewTab || forceReload || audioPreferenceChanged) {
    await setPlayerAudioPreference(tabId, unmuted, record.login.toLowerCase());
  }
  return tabId;
}
  globalThis.TwitchManagedTabs = Object.freeze({
    bindWorkerCallbacks,
    getRecord: (channelId) => snapshotRecord(managedTabs.get(channelId)),
    deleteRecord: (channelId) => managedTabs.delete(channelId),
    recordKeys: () => managedTabs.keys(),
    recordValues: () => Array.from(managedTabs.values(), snapshotRecord),
    recordEntries: () => Array.from(
      managedTabs.entries(),
      ([channelId, record]) => [channelId, snapshotRecord(record)],
    ),
    prepareChannelOpen,
    finishChannelOpen,
    setPlayerWindow,
    removeClosedManagedSurfaces,
    getOwner: (tabId) => managedTabOwners.get(tabId),
    getPendingIdentityCheck: (tabId) => pendingStartupIdentityChecks.get(tabId),
    deletePendingIdentityCheck: (tabId) => pendingStartupIdentityChecks.delete(tabId),
    hasPendingIdentityCheck: (tabId) => pendingStartupIdentityChecks.has(tabId),
    isClosingTab: (tabId) => closingManagedTabIds.has(tabId),
    markClosingTab: (tabId) => closingManagedTabIds.add(tabId),
    unmarkClosingTab: (tabId) => closingManagedTabIds.delete(tabId),
    matchesPendingRoute,
    hasRaidReferrer,
    isCloseOnRaidRedirectEnabled,
    setCloseOnRaidRedirect,
    getRaidRedirectedChannelIds,
    clearRaidRedirectedChannelIds,
    markRaidRedirectedChannel,
    matchesManagedRecordRoute,
    restoreManagedTabs,
    readManagedTabIdentity,
    resolvePendingStartupIdentity,
    invalidateManagedTabOwners,
    saveManagedTabs,
    forgetManagedTabOwner,
    applyManagedPlayerAudioPreference,
    managedSurfaceTabs,
    detachManagedTab,
    openManagedTab,
    activateManagedTab,
    setExistingTabAdoption,
    existingTabAdoptionEnabled: () => adoptExistingTwitchTabs,
    adoptExistingPlayerTab,
    verifyAdoptedTabOwnership,
  });
})();