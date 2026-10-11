const assert = require("node:assert/strict");
const test = require("node:test");
const { channelRoute, externalStreamInventory } = require("./twitch_tabs.js");

test("routes player, website, and chat Twitch tabs", () => {
  assert.deepEqual(
    channelRoute("https://player.twitch.tv/?channel=Example_1&parent=twitch.tv"),
    { surface: "player", login: "example_1", website: false },
  );
  assert.deepEqual(
    channelRoute("https://www.twitch.tv/Example_1"),
    { surface: "player", login: "example_1", website: true },
  );
  assert.deepEqual(
    channelRoute("https://www.twitch.tv/popout/Example_1/chat"),
    { surface: "chat", login: "example_1", website: false },
  );
  assert.equal(channelRoute("https://www.twitch.tv/directory").surface, "");
  assert.equal(channelRoute("https://example.com/example_1").surface, "");
  assert.equal(channelRoute("https://www.twitch.tv/example_1/videos").surface, "");
  assert.equal(channelRoute("https://player.twitch.tv/?video=v123").surface, "");
});

test("counts external player tabs once and excludes managed and chat tabs", () => {
  const inventory = externalStreamInventory(
    [
      { id: 1, url: "https://www.twitch.tv/followed" },
      { id: 2, url: "https://player.twitch.tv/?channel=followed" },
      { id: 3, url: "https://www.twitch.tv/popout/followed/chat" },
      { id: 4, url: "https://www.twitch.tv/other" },
      { id: 5, url: "https://www.twitch.tv/followed/videos" },
    ],
    new Set([1]),
    new Map([["followed", "channel-1"]]),
  );

  assert.deepEqual(inventory, {
    channel_counts: { "channel-1": 1 },
    unmatched_count: 1,
  });
});