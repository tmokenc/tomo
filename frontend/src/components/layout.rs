use yew::prelude::*;
use yew_router::prelude::*;

use crate::components::app::Session;
use crate::routes::Route;

#[derive(Properties, PartialEq)]
pub struct LayoutProps {
    pub children: Children,
}

#[function_component(Layout)]
pub fn layout(props: &LayoutProps) -> Html {
    let session = use_context::<Session>();

    html! {
        <div class="layout">
            <aside class="sidebar">
                <h1>{"Tomo"}</h1>
                <nav>
                    <NavLink to={Route::Home}     label="Dashboard" />
                    <NavLink to={Route::Commands} label="Commands"  />
                    <NavLink to={Route::Triggers} label="Triggers"  />
                </nav>
                <div class="user">
                    if let Some(s) = session {
                        <div>{"Signed in as"}</div>
                        <strong>{ &s.0.username }</strong>
                        <div style="margin-top: 12px;">
                            <a href="/logout">{"Sign out"}</a>
                        </div>
                    }
                </div>
            </aside>
            <main>
                { for props.children.iter() }
            </main>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct NavLinkProps {
    to: Route,
    label: &'static str,
}

#[function_component(NavLink)]
fn nav_link(props: &NavLinkProps) -> Html {
    let route = use_route::<Route>();
    let active = route.as_ref() == Some(&props.to);
    let class = if active { "active" } else { "" };

    html! {
        <Link<Route> to={props.to.clone()} classes={classes!(class)}>
            { props.label }
        </Link<Route>>
    }
}
