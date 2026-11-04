import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";
import "./components/ui/controls.css";
import { TooltipProvider } from "./components/ui/tooltip";

createRoot(document.getElementById("root")!).render(
  <TooltipProvider delayDuration={450} skipDelayDuration={150}>
    <App />
  </TooltipProvider>,
);
