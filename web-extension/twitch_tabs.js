(() => {
  const reservedRoutes = new Set([
    "about",
    "bits",
    "clips",
    "collections",
    "creatorcamp",
    "directory",
    "downloads",
    "drops",
    "inventory",
    "jobs",
    "moderator",
    "p",
    "search",
    "settings",
    "subscriptions",
    "turbo",
    "videos",
    "wallet",
  ]);
  const validLogin = /^[a-z0-9_]{1,25}$/i;

  function channelRoute(value) {
    try {
      const url = new URL(value);
      if (url.protocol !== "https:") return { surface: "", login: "", website: false };
      const host = url.hostname.toLowerCase();
      if (host === "player.twitch.tv") {
        if (url.searchParams.has("video")) {
          return { surface: "", login: "", website: false };
        }
        const login = (url.searchParams.get("channel") || "").toLowerCase();
        return validLogin.test(login)
          ? { surface: "player", login, website: false }
          : { surface: "", login: "", website: false };
      }
      if (host !== "www.twitch.tv") return { surface: "", login: "", website: false };

      const parts = url.pathname.split("/").filter(Boolean);
      if (parts[0]?.toLowerCase() === "popout" && parts[2]?.toLowerCase() === "chat") {
        const login = (parts[1] || "").toLowerCase();
        return validLogin.test(login)
          ? { surface: "chat", login, website: false }
          : { surface: "", login: "", website: false };
      }
      if (parts.length !== 1) return { surface: "", login: "", website: false };
      const login = (parts[0] || "").toLowerCase();
      if (!validLogin.test(login) || reservedRoutes.has(login)) {
        return { surface: "", login: "", website: false };
      }
      return { surface: "player", login, website: true };
    } catch {
      return { surface: "", login: "", website: false };
    }
  }

  function externalStreamInventory(tabs, managedTabIds, channelsByLogin) {
    const channelCounts = {};
    const openStreamerLogins = new Set();
    let unmatchedCount = 0;
    for (const tab of tabs) {
      if (!Number.isInteger(tab.id)) continue;
      const route = channelRoute(tab.url || "");
      if (route.surface !== "player") continue;
      openStreamerLogins.add(route.login.toLowerCase());
      if (managedTabIds.has(tab.id)) continue;
      const channelId = channelsByLogin.get(route.login);
      if (channelId) {
        channelCounts[channelId] = (channelCounts[channelId] || 0) + 1;
      } else {
        unmatchedCount += 1;
      }
    }
    return {
      channel_counts: channelCounts,
      unmatched_count: unmatchedCount,
      open_streamer_logins: Array.from(openStreamerLogins).sort(),
    };

    }

  const api = Object.freeze({ channelRoute, externalStreamInventory });
  globalThis.TwitchTabUtils = api;
  if (typeof module === "object" && module && module.exports) module.exports = api;
})();