//! Root component. Decides whether to show the login page or the layout
//! based on the result of `GET /api/me`.

use std::rc::Rc;

use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::components::commands::CommandsPage;
use crate::components::dashboard::Dashboard;
use crate::components::layout::Layout;
use crate::components::login::Login;
use crate::components::triggers::TriggersPage;
use crate::routes::Route;
use crate::types::Me;

#[derive(Clone, PartialEq)]
pub struct Session(pub Rc<Me>);

#[function_component(App)]
pub fn app() -> Html {
    let session: UseStateHandle<Option<Result<Session, ApiError>>> = use_state(|| None);

    {
        let session = session.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let me = api::me().await.map(|m| Session(Rc::new(m)));
                session.set(Some(me));
            });
        });
    }

    let inner = match session.as_ref() {
        None => html! { <div class="loading">{"Loading…"}</div> },
        Some(Err(ApiError::Unauthorized)) => html! { <Login /> },
        Some(Err(e)) => html! {
            <div class="login-page">
                <h1>{"Tomo"}</h1>
                <div class="error-banner">{format!("Error: {e}")}</div>
                <a href="/login" class="btn">{"Try signing in"}</a>
            </div>
        },
        Some(Ok(s)) => html! {
            <ContextProvider<Session> context={s.clone()}>
                <BrowserRouter>
                    <Switch<Route> render={switch} />
                </BrowserRouter>
            </ContextProvider<Session>>
        },
    };

    inner
}

fn switch(route: Route) -> Html {
    match route {
        Route::Home => html! { <Layout><Dashboard /></Layout> },
        Route::Commands => html! { <Layout><CommandsPage /></Layout> },
        Route::Triggers => html! { <Layout><TriggersPage /></Layout> },
        Route::Login => html! { <Login /> },
        Route::NotFound => html! { <Layout><h2>{"404 — not found"}</h2></Layout> },
    }
}
