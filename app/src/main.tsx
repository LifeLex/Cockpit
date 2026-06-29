import "./app.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";

// INVARIANT: the #root element is always present in index.html.
// eslint-disable-next-line @typescript-eslint/no-non-null-assertion -- root div guaranteed by index.html
const root = document.getElementById("root")!;

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
