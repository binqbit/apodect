import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  BrowserRouter,
  Link,
  NavLink,
  Navigate,
  Outlet,
  useLocation,
  Route,
  Routes,
} from "react-router-dom";
import {
  api,
  ApiError,
  sessionEvents,
  type Account,
  type PublicConfig,
} from "./api";
import { sameAddress, type ConnectedWallet } from "./wallets";
import { Landing } from "./landing";
import { Guide } from "./guide";
import { AccountPage, ProjectsPage } from "./pages";
import { ProjectPage } from "./project";
import { Brand, ErrorState, GuideLink, Loading, short } from "./ui";
import { Icon } from "./icons";

interface SessionContext {
  account: Account | null;
  config: PublicConfig | null;
  loading: boolean;
  error: unknown;
  wallets: ConnectedWallet[];
  setAccount: (account: Account | null) => void;
  rememberWallet: (wallet: ConnectedWallet) => void;
  refreshAccount: () => Promise<void>;
}
const Session = createContext<SessionContext | null>(null);
export function useSession() {
  const value = useContext(Session);
  if (!value) throw new Error("The application session is not available.");
  return value;
}

function SessionProvider({ children }: { children: ReactNode }) {
  const [account, updateAccount] = useState<Account | null>(null);
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [wallets, setWallets] = useState<ConnectedWallet[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>();
  const setAccount = useCallback((value: Account | null) => {
    updateAccount(value);
    if (!value) setWallets([]);
  }, []);
  useEffect(() => {
    const expire = () => setAccount(null);
    sessionEvents.addEventListener("expired", expire);
    return () => sessionEvents.removeEventListener("expired", expire);
  }, [setAccount]);
  const refreshAccount = useCallback(async () => {
    try {
      setAccount(await api.account());
    } catch (failure) {
      if (failure instanceof ApiError && failure.status === 401)
        setAccount(null);
      else throw failure;
    }
  }, []);
  useEffect(() => {
    let current = true;
    Promise.all([api.config(), refreshAccount()])
      .then(
        ([value]) => {
          if (current) setConfig(value);
        },
        (failure: unknown) => {
          if (current) setError(failure);
        },
      )
      .finally(() => {
        if (current) setLoading(false);
      });
    return () => {
      current = false;
    };
  }, [refreshAccount]);
  const rememberWallet = (wallet: ConnectedWallet) =>
    setWallets((previous) => [
      ...previous.filter(
        (item) =>
          item.chain !== wallet.chain ||
          !sameAddress(item.address, wallet.address, wallet.chain),
      ),
      wallet,
    ]);
  return (
    <Session.Provider
      value={{
        account,
        config,
        wallets,
        loading,
        error,
        setAccount,
        rememberWallet,
        refreshAccount,
      }}
    >
      {children}
    </Session.Provider>
  );
}

function WorkspaceLayout() {
  const { account, config } = useSession();
  const location = useLocation();
  const section = location.pathname.split("/")[4];
  const pageName =
    location.pathname === "/app/account"
      ? "Account & wallets"
      : location.pathname.startsWith("/app/projects/")
        ? ({
            tasks: "Tasks & queue",
            funding: "Funding",
            activity: "Activity",
            settings: "Settings",
          }[section ?? ""] ?? "Project overview")
        : "Projects";
  useEffect(() => {
    document.title = `${pageName} · Apodect`;
    return () => {
      document.title = "Apodect";
    };
  }, [pageName]);
  return (
    <div className="app-shell">
      <a className="skip-link" href="#workspace-content">
        Skip to content
      </a>
      <aside className="sidebar">
        <Brand />
        <nav aria-label="Workspace navigation">
          <NavLink
            to="/app"
            end
            className={({ isActive }: { isActive: boolean }) =>
              isActive || location.pathname.startsWith("/app/projects/")
                ? "active"
                : ""
            }
          >
            <span className="nav-icon" aria-hidden="true">
              <Icon name="projects" />
            </span>
            Projects
          </NavLink>
          <NavLink to="/app/account">
            <span className="nav-icon" aria-hidden="true">
              <Icon name="wallet" />
            </span>
            Account & wallets
          </NavLink>
          <GuideLink>
            <span className="nav-icon" aria-hidden="true">
              <Icon name="book" />
            </span>
            User guide
          </GuideLink>
        </nav>
        <div className="sidebar-bottom">
          <Link to="/">How Apodect works ↗</Link>
          <p>
            Your wallet authorizes actions.
            <br />
            Contracts control allocations.
          </p>
        </div>
      </aside>
      <div className="app-main">
        <header className="app-header">
          <div className="header-breadcrumb">
            <Link to="/app">Workspace</Link> <span aria-hidden="true">/</span>
            <strong>{pageName}</strong>
          </div>
          <Link
            to="/app/account"
            state={{
              returnTo: location.pathname.startsWith("/app/projects/")
                ? `${location.pathname}${location.search}${location.hash}`
                : "/app",
            }}
            className={`account-link ${account ? "" : "account-link-guest"}`}
          >
            <span className="avatar" aria-hidden="true">
              {account?.session.profile.display_name
                ?.slice(0, 1)
                .toUpperCase() || <Icon name={account ? "user" : "wallet"} />}
            </span>
            <span className="account-name">
              {account
                ? (account.session.profile.display_name ??
                  short(account.session.wallet.address, 5))
                : "Connect wallet"}
            </span>
          </Link>
        </header>
        {config?.local_simulator && (
          <div className="environment-bar">
            <strong>Local environment</strong>
            <span>Test tokens · Simulated cross-chain delivery</span>
          </div>
        )}
        <main className="app-content" id="workspace-content" tabIndex={-1}>
          <Outlet />
        </main>
      </div>
    </div>
  );
}

function Authenticated({ children }: { children: ReactNode }) {
  const { account, loading, error } = useSession();
  const destination = useLocation();
  if (loading) return <Loading>Connecting to your workspace…</Loading>;
  if (error)
    return <ErrorState error={error} retry={() => location.reload()} />;
  return account ? (
    children
  ) : (
    <Navigate
      to="/app/account"
      replace
      state={{
        returnTo: `${destination.pathname}${destination.search}${destination.hash}`,
      }}
    />
  );
}

function RouteFocus() {
  const { pathname } = useLocation();
  const previousPath = useRef(pathname);
  useEffect(() => {
    if (pathname === "/")
      document.title = "Apodect · Agent budgets across chains";
    if (pathname === previousPath.current) return;
    previousPath.current = pathname;
    if (pathname === "/guide") return;
    const frame = requestAnimationFrame(() => {
      document.querySelector<HTMLElement>("main")?.focus();
      window.scrollTo(0, 0);
    });
    return () => cancelAnimationFrame(frame);
  }, [pathname]);
  return null;
}

export function App() {
  return (
    <BrowserRouter>
      <RouteFocus />
      <SessionProvider>
        <Routes>
          <Route path="/" element={<Landing />} />
          <Route path="/guide" element={<Guide />} />
          <Route
            path="/wallet/callback"
            element={
              <main className="container section" tabIndex={-1}>
                <Brand />
                <h1 style={{ marginTop: 35 }}>Return to your workspace</h1>
                <p className="muted" style={{ marginTop: 15 }}>
                  The wallet response is handled by the window where you started
                  signing. You can close this window.
                </p>
              </main>
            }
          />
          <Route path="/app" element={<WorkspaceLayout />}>
            <Route
              index
              element={
                <Authenticated>
                  <ProjectsPage />
                </Authenticated>
              }
            />
            <Route path="account" element={<AccountPage />} />
            <Route
              path="projects/:projectKey/*"
              element={
                <Authenticated>
                  <ProjectPage />
                </Authenticated>
              }
            />
          </Route>
          <Route
            path="*"
            element={
              <main className="container section" tabIndex={-1}>
                <Brand />
                <h1 style={{ marginTop: 40 }}>Page not found</h1>
                <p className="muted" style={{ margin: "18px 0" }}>
                  This page may have moved. Your projects are available in the
                  workspace.
                </p>
                <Link className="button" to="/app">
                  Go to workspace
                </Link>
              </main>
            }
          />
        </Routes>
      </SessionProvider>
    </BrowserRouter>
  );
}
