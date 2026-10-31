import { mount } from "svelte";
import Setup from "./Setup.svelte";
import "./setup.css";

mount(Setup, {
  target: document.getElementById("app")!,
});
