// The app's version, for a browser build. There is one source: package.json.
//
// In the app itself the version is asked of Tauri at run time. The browser demo
// has no Tauri to ask, so the build hands it this instead of letting it carry a
// number of its own, which is how the demo came to say v0.1.0 two releases
// after that was true.
import { readFileSync } from "node:fs";

export function appVersion() {
  return JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;
}
