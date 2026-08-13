/** React 진입점. 여기서 하는 일은 마운트뿐이다. */

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/base.css";

const root = document.getElementById("root");
if (root === null) throw new Error("#root 를 찾지 못했습니다");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
