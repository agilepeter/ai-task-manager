// Side-effect module: puts the demo backend in place the moment it is
// evaluated, which is before anything that imports the Tauri API runs.
import { install } from "./mock";
install();
