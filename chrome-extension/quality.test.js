const assert = require("node:assert/strict");
const test = require("node:test");
const TwitchQuality = require("./quality.js");

function option(innerText, extra = {}) {
  return {
    innerText,
    disabled: false,
    getAttribute: () => null,
    ...extra,
  };
}

test("missing and unsupported preferences default to 160p", () => {
  assert.equal(TwitchQuality.normalizeQuality(undefined), "160p");
  assert.equal(TwitchQuality.normalizeQuality("ultra"), "160p");
  assert.equal(TwitchQuality.normalizeQuality("720p"), "720p");
});

test("matches a preferred resolution with Twitch's frame-rate suffix", () => {
  const source = option("1080p60 (Source)");
  const option720p = option("720p60");
  const option160p = option("160p");

  assert.equal(TwitchQuality.findOption([source, option720p, option160p], "720p"), option720p);
});

test("matches Twitch's Source rendition label", () => {
  const source = option("1080p60 (Source)");
  const option720p = option("720p60");

  assert.equal(TwitchQuality.findOption([source, option720p], "source"), source);
});

test("falls back to the lowest available video quality when the preference is unavailable", () => {
  const auto = option("Auto");
  const option720p = option("720p60");
  const option360p = option("360p");

  assert.equal(TwitchQuality.findOption([auto, option720p, option360p], "160p"), option360p);
  assert.equal(TwitchQuality.findOption([auto], "160p"), auto);
});

test("uses a sole Source rendition when no numbered quality is available", () => {
  const source = option("Source");

  assert.equal(TwitchQuality.findOption([source], "160p"), source);
});

test("ignores disabled renditions when choosing a fallback", () => {
  const disabled160p = option("160p", { disabled: true });
  const option360p = option("360p");

  assert.equal(TwitchQuality.findOption([disabled160p, option360p], "160p"), option360p);
});
