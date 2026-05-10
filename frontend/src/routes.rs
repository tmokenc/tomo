use yew_router::Routable;

#[derive(Clone, Routable, PartialEq, Debug)]
pub enum Route {
    #[at("/")]
    Home,
    #[at("/commands")]
    Commands,
    #[at("/triggers")]
    Triggers,
    #[at("/login")]
    Login,
    #[not_found]
    #[at("/404")]
    NotFound,
}
