import { render } from "solid-js/web";
import { App } from "./App";
// Bundled, so the app looks the same offline and in the Tauri shell (SIL OFL 1.1).
import "@fontsource-variable/hanken-grotesk/wght.css";
import "@fontsource-variable/bricolage-grotesque/opsz.css";
import "./styles.css";
import "./glass.css";

render(() => <App />, document.getElementById("root")!);
