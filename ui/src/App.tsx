import { lazy, Suspense } from "react";
import { BrowserRouter, Link, Route, Routes } from "react-router";
import { Toaster } from "sonner";
import Layout from "./components/Layout";
import { Empty, HintProvider, Loading } from "./components/ui";
import SearchPage from "./pages/Search";
import RunsPage from "./pages/Runs";

const RunPage = lazy(() => import("./pages/run/RunPage"));
const HowPage = lazy(() => import("./pages/How"));
const InsightsPage = lazy(() => import("./pages/Insights"));
const EnginesPage = lazy(() => import("./pages/Engines"));
const SettingsPage = lazy(() => import("./pages/Settings"));

export default function App() {
  return (
    <HintProvider>
      <BrowserRouter>
        <Suspense fallback={<Loading what="the page" />}>
          <Routes>
            <Route element={<Layout />}>
              <Route index element={<SearchPage />} />
              <Route path="runs" element={<RunsPage />} />
              <Route path="runs/:id" element={<RunPage />} />
              <Route path="runs/:id/:tab" element={<RunPage />} />
              <Route path="how" element={<HowPage />} />
              <Route path="how/:flow" element={<HowPage />} />
              <Route path="insights" element={<InsightsPage />} />
              <Route path="engines" element={<EnginesPage />} />
              <Route path="settings" element={<SettingsPage />} />
              <Route
                path="*"
                element={
                  <Empty title="Nothing here">
                    <Link className="text-accent underline" to="/">
                      Back to Search
                    </Link>
                  </Empty>
                }
              />
            </Route>
          </Routes>
        </Suspense>
      </BrowserRouter>
      <Toaster position="bottom-left" richColors closeButton />
    </HintProvider>
  );
}
