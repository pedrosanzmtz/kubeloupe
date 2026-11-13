import { mount } from "svelte";
import "../app.css";
import Bench from "./Bench.svelte";

// Dev-only page (served by `npm run dev` at /bench.html, not part of the build).
mount(Bench, { target: document.getElementById("app")! });
