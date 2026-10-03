import { getCurrentWindow } from "@tauri-apps/api/window";
import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import Settings from "./Settings.svelte";

// Both windows load this page; the window's label says which one it is.
const settings = getCurrentWindow().label === "settings";
document.documentElement.classList.toggle("settings-window", settings);

const app = mount(settings ? Settings : App, { target: document.getElementById("app")! });

export default app;
