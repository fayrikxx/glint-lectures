import logo from "../assets/logo.svg";
import logoDark from "../assets/logo-dark.svg";
import WindowPicker from "./WindowPicker";

export default function App() {
  return (
    <main className="app">
      <header className="app-header">
        <picture>
          <source srcSet={logoDark} media="(prefers-color-scheme: dark)" />
          <img className="app-logo" src={logo} alt="" width={40} height={40} />
        </picture>
        <h1 className="wordmark">Glint</h1>
      </header>
      <WindowPicker />
    </main>
  );
}
