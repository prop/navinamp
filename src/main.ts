import { mount } from "svelte";
import App from "./App.svelte";
import { installEscapeHandler } from "./lib/window/escape";

installEscapeHandler();

const app = mount(App, {
  target: document.getElementById("app")!,
});

export default app;
