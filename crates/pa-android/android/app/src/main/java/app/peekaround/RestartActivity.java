package app.peekaround;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.os.Process;

/**
 * Relaunches the UI in a fresh process (ProcessPhoenix pattern): runs in ":restart", kills the old
 * UI process, starts MainActivity again and exits.
 */
public class RestartActivity extends Activity {
    private static final String EXTRA_PID = "pid";

    static void restart(Context ctx) {
        Intent i = new Intent(ctx, RestartActivity.class)
                .putExtra(EXTRA_PID, Process.myPid())
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        ctx.startActivity(i);
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        int pid = getIntent().getIntExtra(EXTRA_PID, -1);
        if (pid > 0) Process.killProcess(pid);
        Intent main = new Intent(this, MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK);
        startActivity(main);
        finish();
        Runtime.getRuntime().exit(0);
    }
}
