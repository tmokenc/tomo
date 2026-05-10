use yew::prelude::*;

#[function_component(Login)]
pub fn login() -> Html {
    html! {
        <div class="login-page">
            <h1>{"Tomo"}</h1>
            <p>{"Sign in with Discord to manage the bot."}</p>
            <a href="/login" class="btn" style="background: #5865F2; color: white; padding: 14px 28px; border-radius: 10px; font-weight: 600;">
                {"Continue with Discord"}
            </a>
        </div>
    }
}
