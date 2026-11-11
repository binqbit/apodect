use anchor_lang::prelude::*;

#[error_code]
pub enum ContractError {
    #[msg("Amount must be positive")]
    ZeroAmount,
    #[msg("Purpose reference must be nonzero")]
    EmptyPurpose,
    #[msg("Checked amount or counter arithmetic failed")]
    Arithmetic,
    #[msg("This exact action is not authorized for this signer")]
    Unauthorized,
    #[msg("Task has completed or was canceled")]
    TerminalTask,
    #[msg("Task lifecycle or control state does not permit this action")]
    InvalidTaskState,
    #[msg("A task may have only one live request")]
    LiveRequest,
    #[msg("The provided request does not match the task's live request")]
    WrongLiveRequest,
    #[msg("Invalid request state")]
    InvalidRequestState,
    #[msg("Provided queue head or tail does not match the Project queue")]
    WrongQueueAccount,
    #[msg("Insufficient free Project funds for the complete approved amount")]
    InsufficientProjectFunds,
    #[msg("Amount exceeds the available controlled allocation")]
    InsufficientAllocation,
    #[msg("Return exceeds this task's external outstanding principal")]
    ExcessReturn,
    #[msg("Ordinary cancel is unavailable after first accepted use")]
    AlreadyStarted,
    #[msg("Task must have been funded and release all controlled funds before finalization")]
    CannotFinalize,
    #[msg("Invoice must belong to this Project and be closed for linked funding")]
    InvalidInvoice,
    #[msg("Invoice is already closed")]
    InvoiceClosed,
    #[msg("Project asset must be a six-decimal legacy SPL mint")]
    InvalidMint,
    #[msg("This action belongs to the bound coordinator")]
    CoordinatedProject,
    #[msg("Invalid coordination binding or fresh Project required")]
    InvalidBinding,
    #[msg("Invalid canonical command fields")]
    InvalidCommand,
    #[msg("Invalid command signature")]
    InvalidSignature,
    #[msg("Command has expired")]
    CommandExpired,
    #[msg("Command ID was invalidated or has a different digest")]
    CommandConflict,
    #[msg("Wrong bridge account, identity, or account owner")]
    WrongBridgeAccount,
    #[msg("Plan is missing, closed, allocated, or insufficiently held")]
    InvalidPlan,
    #[msg("Route is missing, registered, incomplete, or inconsistent")]
    InvalidRoute,
    #[msg("Task control nonce does not match")]
    WrongControlNonce,
}
