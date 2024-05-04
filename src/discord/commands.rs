use poise::serenity_prelude as serenity;

pub struct Data {} // User data, which is stored and accessible in all command invocations

type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

/// Edit the caption of some content.
#[poise::command(slash_command, prefix_command)]
pub async fn edit_caption(ctx: Context<'_>, #[description = "Content ID"] message_id: Option<serenity::MessageId>) -> Result<(), Error> {
    let u = message_id.as_ref().unwrap();
    let response = format!("Editing caption for message {}", u);
    ctx.say(response).await?;
    Ok(())
}
