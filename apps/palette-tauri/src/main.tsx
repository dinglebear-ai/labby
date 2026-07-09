import React from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import "./fonts.css";
import "./styles.css";

const rootElement = document.getElementById("app");
if (!rootElement) throw new Error("Missing #app root element");

createRoot(rootElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
