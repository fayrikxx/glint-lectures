import logo from "../assets/logo.svg";
import logoDark from "../assets/logo-dark.svg";

export default function App() {
  return (
    <main className="start">
      <picture>
        <source srcSet={logoDark} media="(prefers-color-scheme: dark)" />
        <img className="start-logo" src={logo} alt="" width={112} height={112} />
      </picture>
      <h1 className="wordmark">Glint</h1>
      <p className="tagline">Catch the key 20% of every lecture.</p>
    </main>
  );
}
