// Entry for the browser demo. Static imports are evaluated in order, and a
// module script runs before DOMContentLoaded, so the backend stand-in is in
// place first and the app still catches the page-ready event it boots on.
// (A dynamic import here lost that race: the app sat at "Starting…".)
import "./install";
import "../main";
