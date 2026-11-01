(() => {
  const DEFAULT_QUALITY = "160p";
  const validQualities = new Set([
    "160p",
    "360p",
    "480p",
    "720p",
    "1080p",
    "source",
    "auto",
  ]);

  function normalizeQuality(value) {
    return validQualities.has(value) ? value : DEFAULT_QUALITY;
  }

  function qualityHeight(label) {
    const match = /^(\d{3,4})p(?:\d+)?(?:\s|$|\()/i.exec(label);
    return match ? Number(match[1]) : null;
  }

  function findOption(options, preference) {
    const items = Array.from(options || [], (option) => ({
      option,
      label: String(option?.innerText ?? option?.textContent ?? "")
        .replace(/\s+/g, " ")
        .trim(),
    })).filter(({ option, label }) =>
      label
      && !option.disabled
      && option.getAttribute?.("aria-disabled") !== "true",
    );
    const quality = normalizeQuality(preference);
    const matches = (label) => {
      if (quality === "auto") return /^auto$/i.test(label);
      if (quality === "source") return /^source$/i.test(label) || /\(\s*source\s*\)$/i.test(label);
      return qualityHeight(label) === Number.parseInt(quality, 10);
    };
    const requested = items.find(({ label }) => matches(label));
    if (requested) return requested.option;

    const lowest = items
      .map(({ option, label }) => ({ option, height: qualityHeight(label) }))
      .filter(({ height }) => height !== null)
      .sort((left, right) => left.height - right.height)[0];
    return lowest?.option
      ?? items.find(({ label }) => /^source$/i.test(label) || /\(\s*source\s*\)$/i.test(label))?.option
      ?? items.find(({ label }) => /^auto$/i.test(label))?.option
      ?? null;
  }

  const api = Object.freeze({ DEFAULT_QUALITY, normalizeQuality, findOption });
  globalThis.TwitchQuality = api;
  if (typeof module === "object" && module && module.exports) module.exports = api;
})();
