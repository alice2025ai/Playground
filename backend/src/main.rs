mod telegram_client;
mod block_chain;

use std::env;
use actix_cors::Cors;
use actix_web::{App, HttpServer,HttpResponse, post, web,Responder};
// main.rs
use teloxide::{prelude::*};
use dotenv::dotenv;
use reqwest::Url;
use teloxide::types::{ChatMemberKind, InlineKeyboardButton, InlineKeyboardMarkup};
use ethers::{
    prelude::*,
    utils::hash_message,
};
use ethers::utils::hex;
use reqwest::Client;
use std::str::FromStr;
use ethers::abi::Abi;
use std::sync::Arc;
const ABI: &str = r#"[	{
		"inputs": [
			{
				"internalType": "address",
				"name": "",
				"type": "address"
			},
			{
				"internalType": "address",
				"name": "",
				"type": "address"
			}
		],
		"name": "sharesBalance",
		"outputs": [
			{
				"internalType": "uint256",
				"name": "",
				"type": "uint256"
			}
		],
		"stateMutability": "view",
		"type": "function"
	}]"#;


#[derive(Clone)]
struct AppConfig {
    telegram_bot_token: String,
    telegram_group_id: String,
    shares_contract: String,
    chain_rpc: String,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ChallengeRequest {
    pub challenge: String,
    pub signature: String,
    pub shares_subject: String,
    pub user: String,
}

#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn verify_signature(
    challenge: &str,
    signature: &str,
) -> Result<Address, String> {
    let sig_bytes = hex::decode(signature)
        .map_err(|e| format!("Invalid signature hex: {}", e))?;

    if sig_bytes.len() != 65 {
        return Err("Signature must be 65 bytes".into());
    }

    let message_hash = hash_message(challenge);
    let signature = Signature::try_from(sig_bytes.as_slice()).map_err(|e| format!("Invalid signature: {}!",e))?;
    let recovered_address = signature
        .recover(message_hash)
        .map_err(|e| format!("Recovery failed: {}", e))?;
    Ok(recovered_address)
}


#[post("/verify-signature")]
async fn handle_verify(
    data: web::Json<ChallengeRequest>,
    config: web::Data<AppConfig>,
) -> impl Responder {

    let own_shares = match verify_signature(
        &data.challenge,
        // &data.address,
        &data.signature,
    ) {
        Ok(address) => {
            println!("Verified address is {}",address.to_string());
            let user_address = Address::from_str(&data.user).expect("Invalid user address");
            if user_address == address {
                let provider = Provider::<Http>::try_from(&config.chain_rpc).expect("Connect monad failed");
                let contract_address = Address::from_str(&config.shares_contract).expect("Invalid contract");
                let abi: ethers::abi::Abi = serde_json::from_str(ABI).expect("Invalid abi");
                let contract = ethers::contract::Contract::new(
                    contract_address,
                    abi,
                    Arc::new(provider)
                );

                let subject_address = Address::from_str(&data.shares_subject).expect("Invalid subject address");

                let balance: U256 = contract
                    .method::<_, U256>("sharesBalance", (subject_address, user_address)).expect("Get method failed")
                    .call()
                    .await.expect("Call sharesBalance failed");

                println!("Balance: {}", balance);
                !balance.is_zero()
            } else {
                println!("Address mismatch with signature!");
                false
            }
        }
        Err(e) => {
            println!("Verify signature failed: {:?}",e);
            false
        },
    };
    if !own_shares {
        let client = Client::new();
        let url = format!(
            "https://api.telegram.org/bot{}/banChatMember",
            config.telegram_bot_token
        );
        let params = [
            ("chat_id", &config.telegram_group_id),
            ("user_id", &data.challenge),
        ];

        println!("url is {},params is {:?}",url,params);
        match client.post(&url).form(&params).send().await {
            Ok(resp) => {
                println!("resp is {:?}",resp.status());
                if !resp.status().is_success() {
                    return HttpResponse::InternalServerError().json(ChallengeResponse {
                        success: false,
                        error: Some(format!("Telegram API call failed",)),
                    });
                }
            }
            Err(e) => {
                println!("Verified signature failed: {:?}",e);
                return HttpResponse::InternalServerError().json(ChallengeResponse {
                    success: false,
                    error: Some(format!("Telegram request failed: {}", e)),
                });
            },
        }
        let url = format!(
            "https://api.telegram.org/bot{}/unbanChatMember",
            config.telegram_bot_token
        );
        let ret = client.post(&url).form(&params).send().await.unwrap();
        println!("unban chat member ret {:?}",ret);
        return HttpResponse::Ok().json(ChallengeResponse {
            success: false,
            error: None,
        });
    }

    HttpResponse::Ok().json(ChallengeResponse {
        success: true,
        error: None,
    })
}
#[tokio::main]
async fn main() {
    dotenv().ok();
    let config = AppConfig {
        telegram_bot_token: env::var("TELEGRAM_BOT_TOKEN")
            .expect("TELEGRAM_BOT_TOKEN not set"),
        telegram_group_id: env::var("TELEGRAM_GROUP_ID")
            .expect("TELEGRAM_GROUP_ID not set"),
        shares_contract: env::var("SHARES_CONTRACT_ADDRESS")
            .expect("SHARES_CONTRACT_ADDRESS not set"),
        chain_rpc: env::var("CHAIN_RPC")
            .expect("CHAIN_RPC not set"),
    };
    //let provider = Provider::<Http>::try_from(&config.chain_rpc).expect("Connect monad failed");
    let bot = Bot::new(&config.telegram_bot_token);
    let http_server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(cors)
            .app_data(web::Data::new(config.clone()))
            .service(handle_verify)
    })
        .bind("127.0.0.1:8080").unwrap()
        .run();

    let (res1, res2) = tokio::join!(
        async {
            http_server.await.expect("TODO: panic message");
        },
        async {
            teloxide::repl(bot, |bot: Bot, msg: Message| async move {
                if let Some(new_chat_member) = msg.new_chat_members() {
                    for user in new_chat_member {
                        println!(
                            "[newChatMember] chat ID: {}, user ID: {}, user name: @{}",
                            msg.chat.id,
                            user.id,
                            user.username.as_deref().unwrap_or("nick user")
                        );
                        // let user_status = bot.get_chat_member(msg.chat.id,user.id).await?.kind;
                        // if matches!(user_status,ChatMemberKind::Member) {
                        //     bot.send_message(user.id, "").await.unwrap();
                        // }
                        // let ret = bot.ban_chat_member(msg.chat.id,user.id).await.unwrap();
                        // println!("bot ban chat member ret: {:?}",ret);
                        let url_str = format!("http://127.0.0.1:8000/sign.html?challenge={}", user.id);
                        let url = Url::parse(&url_str).unwrap();
                        let keyboard = InlineKeyboardMarkup::new(
                            vec![vec![
                                InlineKeyboardButton::url(
                                    "ClickToSign",
                                     url,
                                )
                            ]]
                        );

                        bot.send_message(user.id, "Please sign to verify wallet ownership:")
                            .reply_markup(keyboard)
                            .await.unwrap();
                    }
                }

                if let Some(user) = msg.left_chat_member() {
                    println!(
                        "[MemberLeft] chat ID: {}, user ID: {}, user name: @{}",
                        msg.chat.id,
                        user.id,
                        user.username.as_deref().unwrap_or("nick user")
                    )
                }

                respond(())
            }).await;
        }
    );

    //res1.expect("Server error");
    //res2.expect("Bot error");
}