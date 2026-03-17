#!/usr/bin/env node
"use strict";

const fs = require("fs");
const https = require("https");
const http = require("http");
const path = require("path");
const os = require("os");

const REPO = "denyzhirkov/kungfu";
const VERSION = require("./package.json").version;

function getPlatform() {
  const platform = os.platform();
  const arch = os.arch();

  const platformMap = {
    darwin: "darwin",
    linux: "linux",
    win32: "windows",
  };

  const archMap = {
    x64: "x86_64",
    arm64: "aarch64",
  };

  const p = platformMap[platform];
  const a = archMap[arch];

  if (!p || !a) {
    console.error(`Unsupported platform: ${platform}-${arch}`);
    process.exit(1);
  }

  const ext = platform === "win32" ? ".exe" : "";
  return { name: `kungfu-${p}-${a}${ext}`, ext };
}

function getCacheDir() {
  const home = os.homedir();
  if (os.platform() === "win32") {
    return path.join(process.env.LOCALAPPDATA || path.join(home, "AppData", "Local"), "kungfu", "bin");
  }
  return path.join(process.env.XDG_CACHE_HOME || path.join(home, ".cache"), "kungfu", "bin");
}

function download(url) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith("https") ? https : http;
    client
      .get(url, { headers: { "User-Agent": "kungfu-ai-npm" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return download(res.headers.location).then(resolve).catch(reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", reject);
  });
}

async function main() {
  const { name, ext } = getPlatform();
  const tag = `v${VERSION}`;
  const url = `https://github.com/${REPO}/releases/download/${tag}/${name}`;

  const binDir = path.join(__dirname, "bin");
  const binPath = path.join(binDir, `kungfu${ext}`);

  // Check persistent cache first (~/.cache/kungfu/bin/kungfu-v1.0.6)
  const cacheDir = getCacheDir();
  const cachedBin = path.join(cacheDir, `kungfu-${tag}${ext}`);

  if (fs.existsSync(cachedBin)) {
    const stat = fs.statSync(cachedBin);
    if (stat.size > 1000) {
      // Copy from cache to local bin
      if (!fs.existsSync(binDir)) {
        fs.mkdirSync(binDir, { recursive: true });
      }
      fs.copyFileSync(cachedBin, binPath);
      fs.chmodSync(binPath, 0o755);
      return;
    }
  }

  // Skip if binary already exists locally
  if (fs.existsSync(binPath)) {
    const stat = fs.statSync(binPath);
    if (stat.size > 1000) {
      return;
    }
  }

  console.log(`kungfu: downloading ${name}...`);

  try {
    const data = await download(url);

    if (!fs.existsSync(binDir)) {
      fs.mkdirSync(binDir, { recursive: true });
    }

    fs.writeFileSync(binPath, data);
    fs.chmodSync(binPath, 0o755);

    // Save to persistent cache for future npx runs
    try {
      if (!fs.existsSync(cacheDir)) {
        fs.mkdirSync(cacheDir, { recursive: true });
      }
      fs.copyFileSync(binPath, cachedBin);
    } catch (_) {
      // Cache write failure is non-fatal
    }

    console.log(`kungfu: installed to ${binPath}`);
  } catch (err) {
    console.error(`kungfu: failed to download binary from ${url}`);
    console.error(`  ${err.message}`);
    console.error(`  Check releases: https://github.com/${REPO}/releases`);
    process.exit(1);
  }
}

main();
